//! Intl.RelativeTimeFormat (ECMA-402 §17) over the CLDR 46 dateFields of the locale (INTLCLDR, SR137): the
//! relative phrases ("yesterday", "vorgestern") for numeric "auto", otherwise the future / past pattern of the
//! number's plural category around the number the locale's NumberFormat writes.

use super::number::{format_parts as num_parts, Dec, Digits, Mv, NumFmt};
use super::*;
use crate::cldr::L;

#[derive(Clone, Debug)]
pub struct RelFmt {
    pub locale: String,
    pub loc: L,
    pub ns: &'static str,
    pub style: &'static str,
    pub numeric: &'static str,
}

const UNITS: [&str; 8] = ["second", "minute", "hour", "day", "week", "month", "quarter", "year"];

/// SingularRelativeTimeUnit (§17.5.1).
fn singular(u: &str) -> Option<&'static str> {
    let s = u.strip_suffix('s').unwrap_or(u);
    UNITS.iter().copied().find(|x| *x == s)
}

fn width(style: &str) -> usize {
    match style {
        "short" => 1,
        "narrow" => 2,
        _ => 0,
    }
}

fn field(unit: &str) -> usize {
    crate::cldr::FIELDS.iter().position(|x| *x == unit).unwrap_or(0)
}

/// PartitionRelativeTimePattern (§17.5.2): the parts and, per part, the unit it carries ("" for literals).
pub fn parts(f: &RelFmt, v: f64, unit: &'static str) -> (Vec<(String, String)>, Vec<String>) {
    let fi = field(unit);
    let w = width(f.style);
    if f.numeric == "auto" && v == crate::numconv::libm_floor(v) && (-2.0..=2.0).contains(&v) {
        let k = (v as i64 + 2) as usize;
        if let Some(ph) = f.loc.relative(fi, w, k) {
            return (vec![(String::from("literal"), String::from(ph))], vec![String::new()]);
        }
    }
    let past = v < 0.0 || (v == 0.0 && v.is_sign_negative());
    let nf = NumFmt { sign: String::from("never"), ..NumFmt::plain(f.loc, &f.locale, f.ns, Digits::fixed(0, 3)) };
    let x = Mv::Fin(Dec::from_f64(v.abs()));
    let np = num_parts(&nf, &x);
    let (int, frac): (String, String) = {
        let mut i = String::new();
        let mut fr = String::new();
        // the plural operands come from the ASCII digits of the value as formatted
        let ascii = |s: &str| -> String { s.chars().filter_map(|c| c.to_digit(10).map(|d| (b'0' + d as u8) as char)).collect() };
        for (t, s) in &np {
            if t == "integer" {
                i.push_str(&ascii(s));
            } else if t == "fraction" {
                fr.push_str(&ascii(s));
            }
        }
        (i, fr)
    };
    let cat = super::number::plural_of(&nf, &int, &frac, 0);
    let ci = crate::cldr::cat_index(cat);
    let base = if past { 11 } else { 5 };
    let pat = f
        .loc
        .relative(fi, w, base + ci)
        .or_else(|| f.loc.relative(fi, w, base + 5))
        .or_else(|| f.loc.relative(fi, 0, base + ci))
        .or_else(|| f.loc.relative(fi, 0, base + 5))
        .unwrap_or("{0}");
    let (pre, post) = pat.split_once("{0}").unwrap_or(("", ""));
    let mut out = Vec::new();
    let mut units = Vec::new();
    if !pre.is_empty() {
        out.push((String::from("literal"), String::from(pre)));
        units.push(String::new());
    }
    for p in np {
        out.push(p);
        units.push(String::from(unit));
    }
    if !post.is_empty() {
        out.push((String::from("literal"), String::from(post)));
        units.push(String::new());
    }
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
    let nu = get_string_option(vm, o, "numberingSystem", &[], svc)?;
    if let Some(ns) = &nu {
        if !ns.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid numberingSystem : {ns}"));
        }
    }
    let r = resolve_locale(&requested, &["nu"]);
    let (ns, kept) = numbering_system(&r, nu.as_deref());
    let style = get_string_option(vm, o, "style", &["long", "short", "narrow"], svc)?;
    let numeric = get_string_option(vm, o, "numeric", &["always", "auto"], svc)?;
    let f = RelFmt {
        locale: if kept { r.locale_with(&[("nu", ns)]) } else { String::from(r.data_locale) },
        loc: r.loc,
        ns,
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
            ("numberingSystem", Value::str(f.ns)),
        ],
    );
    Ok(Value::Object(r))
}
