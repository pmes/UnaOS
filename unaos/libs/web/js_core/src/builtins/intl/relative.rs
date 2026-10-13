//! Intl.RelativeTimeFormat (ECMA-402 §17) with the CLDR `en` relative-time patterns.

use super::number::{format_parts as num_parts, Digits, Dec, Mv, NumFmt};
use super::*;

#[derive(Clone, Debug)]
pub struct RelFmt {
    pub locale: String,
    pub style: &'static str,
    pub numeric: &'static str,
}

const UNITS: [&str; 8] = ["second", "minute", "hour", "day", "week", "month", "quarter", "year"];

/// SingularRelativeTimeUnit (§17.5.1).
fn singular(u: &str) -> Option<&'static str> {
    let s = u.strip_suffix('s').unwrap_or(u);
    UNITS.iter().copied().find(|x| *x == s)
}

fn auto_phrase(unit: &str, v: f64, style: &str) -> Option<&'static str> {
    let k = if v == 0.0 { 0 } else if v == 1.0 { 1 } else if v == -1.0 { -1 } else { return None };
    let long = style == "long";
    Some(match (unit, k) {
        ("second", 0) => "now",
        ("minute", 0) => "this minute",
        ("hour", 0) => "this hour",
        ("day", -1) => "yesterday",
        ("day", 0) => "today",
        ("day", 1) => "tomorrow",
        ("week", -1) => if long { "last week" } else { "last wk." },
        ("week", 0) => if long { "this week" } else { "this wk." },
        ("week", 1) => if long { "next week" } else { "next wk." },
        ("month", -1) => if long { "last month" } else { "last mo." },
        ("month", 0) => if long { "this month" } else { "this mo." },
        ("month", 1) => if long { "next month" } else { "next mo." },
        ("quarter", -1) => if long { "last quarter" } else { "last qtr." },
        ("quarter", 0) => if long { "this quarter" } else { "this qtr." },
        ("quarter", 1) => if long { "next quarter" } else { "next qtr." },
        ("year", -1) => if long { "last year" } else { "last yr." },
        ("year", 0) => if long { "this year" } else { "this yr." },
        ("year", 1) => if long { "next year" } else { "next yr." },
        _ => return None,
    })
}

/// The unit word after the number (long / short) or glued to it (narrow).
fn unit_word(unit: &str, style: &str, one: bool) -> (&'static str, &'static str) {
    match style {
        "long" => (
            " ",
            match (unit, one) {
                ("second", true) => "second",
                ("second", false) => "seconds",
                ("minute", true) => "minute",
                ("minute", false) => "minutes",
                ("hour", true) => "hour",
                ("hour", false) => "hours",
                ("day", true) => "day",
                ("day", false) => "days",
                ("week", true) => "week",
                ("week", false) => "weeks",
                ("month", true) => "month",
                ("month", false) => "months",
                ("quarter", true) => "quarter",
                ("quarter", false) => "quarters",
                ("year", true) => "year",
                _ => "years",
            },
        ),
        "short" => (
            " ",
            match (unit, one) {
                ("second", _) => "sec.",
                ("minute", _) => "min.",
                ("hour", _) => "hr.",
                ("day", true) => "day",
                ("day", false) => "days",
                ("week", _) => "wk.",
                ("month", _) => "mo.",
                ("quarter", true) => "qtr.",
                ("quarter", false) => "qtrs.",
                _ => "yr.",
            },
        ),
        _ => (
            "",
            match unit {
                "second" => "s",
                "minute" => "m",
                "hour" => "h",
                "day" => "d",
                "week" => "w",
                "month" => "mo",
                "quarter" => "q",
                _ => "y",
            },
        ),
    }
}

/// PartitionRelativeTimePattern (§17.5.2): the parts and, per part, the unit it carries ("" for literals).
pub fn parts(f: &RelFmt, v: f64, unit: &'static str) -> (Vec<(String, String)>, Vec<String>) {
    if f.numeric == "auto" {
        if let Some(ph) = auto_phrase(unit, v, f.style) {
            return (vec![(String::from("literal"), String::from(ph))], vec![String::new()]);
        }
    }
    let past = v < 0.0 || (v == 0.0 && v.is_sign_negative());
    let loc = loc_of(&f.locale);
    let nf = NumFmt { sign: String::from("never"), ..NumFmt::plain(loc, &f.locale, loc.default_ns(), Digits::fixed(0, 3)) };
    let x = Mv::Fin(Dec::from_f64(v.abs()));
    let np = num_parts(&nf, &x);
    let (int, frac): (String, String) = np.iter().fold((String::new(), String::new()), |(mut i, mut fr), (t, s)| {
        if t == "integer" {
            i.push_str(s);
        } else if t == "fraction" {
            fr.push_str(s);
        }
        (i, fr)
    });
    let one = int == "1" && frac.is_empty();
    let (gap, word) = unit_word(unit, f.style, one);
    let mut out = Vec::new();
    let mut units = Vec::new();
    if !past {
        out.push((String::from("literal"), String::from("in ")));
        units.push(String::new());
    }
    for p in np {
        out.push(p);
        units.push(String::from(unit));
    }
    let tail = if past { format!("{gap}{word} ago") } else { format!("{gap}{word}") };
    out.push((String::from("literal"), tail));
    units.push(String::new());
    (out, units)
}

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "RelativeTimeFormat", 0, rtf_ctor);
    method(vm, p, "format", 2, rtf_format);
    method(vm, p, "formatToParts", 2, rtf_format_to_parts);
    method(vm, p, "resolvedOptions", 0, rtf_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_relative_time_format_proto = p;
}

fn rtf_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "RelativeTimeFormat")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_relative_time_format_proto)?;
    let l = vm.arg(ctx, 0);
    let requested = canonicalize_locale_list(vm, &l)?;
    let ov = vm.arg(ctx, 1);
    let o = coerce_options(vm, &ov)?;
    let svc = "Intl.RelativeTimeFormat";
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], svc)?;
    if let Some(ns) = get_string_option(vm, o, "numberingSystem", &[], svc)? {
        if !ns.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid numberingSystem : {ns}"));
        }
    }
    let r = resolve_locale(&requested, &["nu"]);
    let style = get_string_option(vm, o, "style", &["long", "short", "narrow"], svc)?;
    let numeric = get_string_option(vm, o, "numeric", &["always", "auto"], svc)?;
    let f = RelFmt {
        locale: String::from(r.data_locale),
        style: match style.as_deref() {
            Some("short") => "short",
            Some("narrow") => "narrow",
            _ => "long",
        },
        numeric: if numeric.as_deref() == Some("auto") { "auto" } else { "always" },
    };
    Ok(Value::Object(alloc_intl(vm, p, IntlData::RelativeTimeFormat(Box::new(f)))))
}

fn is_rtf(d: &IntlData) -> bool {
    matches!(d, IntlData::RelativeTimeFormat(_))
}

fn args(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<(RelFmt, f64, &'static str)> {
    let o = this_intl(vm, &ctx.this, is_rtf, what)?;
    let a = vm.arg(ctx, 0);
    let v = vm.to_number(&a)?;
    let b = vm.arg(ctx, 1);
    let u = vm.to_string(&b)?.to_rust();
    if !v.is_finite() {
        return vm.throw_range("Invalid value: the value must be finite");
    }
    let unit = match singular(&u) {
        Some(x) => x,
        None => return vm.throw_range(&format!("Invalid unit argument for format() '{u}'")),
    };
    let f = match intl_ref(vm, o) {
        IntlData::RelativeTimeFormat(f) => (**f).clone(),
        _ => unreachable!(),
    };
    Ok((f, v, unit))
}

fn rtf_format(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (f, v, unit) = args(vm, ctx, "Intl.RelativeTimeFormat.prototype.format")?;
    let (ps, _) = parts(&f, v, unit);
    Ok(Value::str(&concat_parts(&ps)))
}

fn rtf_format_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (f, v, unit) = args(vm, ctx, "Intl.RelativeTimeFormat.prototype.formatToParts")?;
    let (ps, units) = parts(&f, v, unit);
    Ok(parts_array(vm, &ps, Some(("unit", &units))))
}

fn rtf_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_rtf, "Intl.RelativeTimeFormat.prototype.resolvedOptions")?;
    let f = match intl_ref(vm, o) {
        IntlData::RelativeTimeFormat(f) => (**f).clone(),
        _ => unreachable!(),
    };
    let r = object_from(
        vm,
        vec![
            ("locale", Value::str(&f.locale)),
            ("style", Value::str(f.style)),
            ("numeric", Value::str(f.numeric)),
            ("numberingSystem", Value::str("latn")),
        ],
    );
    Ok(Value::Object(r))
}
