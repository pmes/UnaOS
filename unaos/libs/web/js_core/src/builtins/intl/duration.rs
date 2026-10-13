//! Intl.DurationFormat (ECMA-402 2025 §DurationFormat) for `en`: style long / short / narrow / digital,
//! per-unit styles and displays, fractionalDigits; format / formatToParts / resolvedOptions. Unit text
//! comes from the NumberFormat unit patterns (`data::UNITS`), the list from ListFormat's `unit` type.

use super::number::{format_parts as num_parts, Dec, Digits, Mv, NumFmt, Notation, Style};
use super::*;

pub const UNITS: [&str; 10] =
    ["years", "months", "weeks", "days", "hours", "minutes", "seconds", "milliseconds", "microseconds", "nanoseconds"];

#[derive(Clone, Debug)]
pub struct DurFmt {
    pub locale: String,
    pub style: &'static str,
    /// Per unit: (style, display "auto" / "always").
    pub units: [(&'static str, &'static str); 10],
    pub fractional_digits: Option<u32>,
}

fn singular(u: &str) -> &str {
    u.strip_suffix('s').unwrap_or(u)
}

const VALUES_DATE: &[&str] = &["long", "short", "narrow"];
const VALUES_TIME: &[&str] = &["long", "short", "narrow", "numeric", "2-digit"];
const VALUES_SUB: &[&str] = &["long", "short", "narrow", "numeric"];

pub fn create(vm: &mut Vm, locales: &Value, options: &Value) -> JsResult<DurFmt> {
    let requested = canonicalize_locale_list(vm, locales)?;
    let o = options_object(vm, options)?;
    let svc = "Intl.DurationFormat";
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], svc)?;
    if let Some(ns) = get_string_option(vm, o, "numberingSystem", &[], svc)? {
        if !ns.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid numberingSystem : {ns}"));
        }
    }
    let r = resolve_locale(&requested, &["nu"]);
    let style = get_string_option(vm, o, "style", &["long", "short", "narrow", "digital"], svc)?;
    let style: &'static str = match style.as_deref() {
        Some("long") => "long",
        Some("narrow") => "narrow",
        Some("digital") => "digital",
        _ => "short",
    };
    let mut units = [("short", "auto"); 10];
    let mut prev: &'static str = "";
    let numeric_like = |s: &str| matches!(s, "numeric" | "2-digit");
    for (i, u) in UNITS.iter().enumerate() {
        // GetDurationUnitOptions.
        let values: &[&str] = match i {
            0..=3 => VALUES_DATE,
            4..=6 => VALUES_TIME,
            _ => VALUES_SUB,
        };
        let opt = get_string_option(vm, o, u, values, svc)?;
        let mut display_default = "always";
        let mut st: &'static str = match opt.as_deref() {
            Some("long") => "long",
            Some("short") => "short",
            Some("narrow") => "narrow",
            Some("numeric") => "numeric",
            Some(_) => "2-digit",
            None => {
                if style == "digital" {
                    if !(4..=6).contains(&i) {
                        display_default = "auto";
                    }
                    if i <= 3 { "short" } else { "numeric" }
                } else if numeric_like(prev) || prev == "fractional" {
                    if !(i == 5 || i == 6) {
                        display_default = "auto";
                    }
                    "numeric"
                } else {
                    display_default = "auto";
                    style
                }
            }
        };
        if st == "numeric" && i >= 7 {
            st = "fractional";
            display_default = "auto";
        }
        let disp = get_string_option(vm, o, &format!("{}Display", singular(u)), &["auto", "always"], svc)?;
        let disp: &'static str = match disp.as_deref() {
            Some("always") => "always",
            Some(_) => "auto",
            None => display_default,
        };
        if disp == "always" && st == "fractional" {
            return vm.throw_range(&format!("{u}Display \"always\" conflicts with a fractional {u}"));
        }
        if prev == "fractional" && st != "fractional" {
            return vm.throw_range(&format!("{u} must be numeric after a fractional unit"));
        }
        if numeric_like(prev) && !(numeric_like(st) || st == "fractional") {
            return vm.throw_range(&format!("{u} must be numeric after a numeric unit"));
        }
        if (i == 5 || i == 6) && numeric_like(prev) {
            st = "2-digit";
        }
        units[i] = (st, disp);
        prev = st;
    }
    let fd = get_number_option(vm, o, "fractionalDigits", 0.0, 9.0)?;
    Ok(DurFmt { locale: String::from(r.data_locale), style, units, fractional_digits: fd })
}

/// ToDurationRecord + IsValidDuration (§ "duration records"): integers, one sign, at least one field.
fn to_record(vm: &mut Vm, v: &Value) -> JsResult<[f64; 10]> {
    let o = match v {
        Value::Object(o) => *o,
        Value::String(_) => return vm.throw_range("Invalid duration: strings are not accepted"),
        _ => return vm.throw_type("Invalid duration: an object is required"),
    };
    let mut rec = [0.0f64; 10];
    let mut any = false;
    // Read in the spec's (alphabetical) order.
    let mut order: Vec<usize> = (0..10).collect();
    order.sort_by_key(|&i| UNITS[i]);
    for i in order {
        let x = vm.get(o, &PropertyKey::from_str(UNITS[i]))?;
        if x.is_undefined() {
            continue;
        }
        any = true;
        let n = vm.to_number(&x)?;
        if !n.is_finite() || n != crate::numconv::libm_floor(n) && n != -crate::numconv::libm_floor(-n) {
            return vm.throw_range(&format!("Invalid duration: {} must be an integer", UNITS[i]));
        }
        rec[i] = n;
    }
    if !any {
        return vm.throw_type("Invalid duration: no duration fields");
    }
    let pos = rec.iter().any(|&x| x > 0.0);
    let neg = rec.iter().any(|&x| x < 0.0);
    if pos && neg {
        return vm.throw_range("Invalid duration: mixed signs");
    }
    Ok(rec)
}

fn plain_nf(locale: &str, min_int: u32, max_frac: u32, min_frac: u32) -> NumFmt {
    NumFmt {
        locale: String::from(locale),
        gb: false,
        style: Style::Decimal,
        currency: String::new(),
        currency_display: String::from("symbol"),
        currency_sign: String::from("standard"),
        unit: String::new(),
        unit_display: String::from("short"),
        dg: Digits { min_int, ..Digits::fixed(min_frac, max_frac) },
        notation: Notation::Standard,
        compact_long: false,
        grouping: Some("auto"),
        sign: String::from("auto"),
    }
}

/// PartitionDurationFormatPattern: (parts, per part the unit it formats — "" for separators).
pub fn parts(f: &DurFmt, rec: &[f64; 10]) -> (Vec<(String, String)>, Vec<String>) {
    let mut groups: Vec<(Vec<(String, String)>, Vec<String>)> = Vec::new();
    let neg = rec.iter().any(|&x| x < 0.0);
    let mut sign_shown = false;
    let mut i = 0;
    // Numeric milliseconds fold every sub-second unit into the seconds' fraction.
    let fold_sub = f.units[7].0 == "fractional";
    while i < 10 {
        let (st, disp) = f.units[i];
        let v = rec[i];
        if (4..=6).contains(&i) && matches!(st, "numeric" | "2-digit") {
            // A run of numeric time units "h:mm:ss[.fff]": once one is shown, the rest follow it.
            let mut run: Vec<(String, String)> = Vec::new();
            let mut ru: Vec<String> = Vec::new();
            let mut shown = false;
            let mut j = i;
            while j <= 6 && matches!(f.units[j].0, "numeric" | "2-digit") {
                let sub = j == 6 && fold_sub && (rec[7] != 0.0 || rec[8] != 0.0 || rec[9] != 0.0);
                let mut val = rec[j].abs();
                if j == 6 && fold_sub {
                    val += rec[7].abs() / 1e3 + rec[8].abs() / 1e6 + rec[9].abs() / 1e9;
                }
                if shown || rec[j] != 0.0 || f.units[j].1 == "always" || sub {
                    if shown {
                        run.push((String::from("literal"), String::from(":")));
                        ru.push(String::new());
                    }
                    let two = f.units[j].0 == "2-digit";
                    let (maxf, minf) = if j == 6 { (f.fractional_digits.unwrap_or(9), f.fractional_digits.unwrap_or(0)) } else { (0, 0) };
                    let nf = NumFmt { grouping: None, ..plain_nf(&f.locale, if two { 2 } else { 1 }, maxf, minf) };
                    let mut np = num_parts(&nf, &Mv::Fin(Dec::from_f64(val)));
                    if neg && !sign_shown {
                        np.insert(0, (String::from("minusSign"), String::from("-")));
                        sign_shown = true;
                    }
                    for p in np {
                        run.push(p);
                        ru.push(String::from(singular(UNITS[j])));
                    }
                    shown = true;
                }
                j += 1;
            }
            if shown {
                groups.push((run, ru));
            }
            i = if j == 7 && fold_sub { 10 } else { j };
            continue;
        }
        if i >= 7 && fold_sub {
            break;
        }
        let sub = if i == 6 && fold_sub { rec[7].abs() / 1e3 + rec[8].abs() / 1e6 + rec[9].abs() / 1e9 } else { 0.0 };
        let v = if sub != 0.0 { v.abs() + sub } else { v };
        if v == 0.0 && disp == "auto" {
            i += 1;
            continue;
        }
        let (maxf, minf) = if i == 6 && fold_sub { (f.fractional_digits.unwrap_or(9), f.fractional_digits.unwrap_or(0)) } else { (0, 0) };
        let nf = plain_nf(&f.locale, 1, maxf, minf);
        let mut np = num_parts(&nf, &Mv::Fin(Dec::from_f64(v.abs())));
        if neg && !sign_shown {
            np.insert(0, (String::from("minusSign"), String::from("-")));
            sign_shown = true;
        }
        let int: String = np.iter().filter(|p| p.0 == "integer").map(|p| p.1.as_str()).collect();
        let frac: String = np.iter().filter(|p| p.0 == "fraction").map(|p| p.1.as_str()).collect();
        let one = int == "1" && frac.is_empty();
        let unit = data::unit(singular(UNITS[i]));
        let pat: &str = match (unit, st) {
            (Some(u), "long") => {
                if one {
                    u.long_one
                } else {
                    u.long_other
                }
            }
            (Some(u), "narrow") => u.narrow,
            (Some(u), _) => {
                if one {
                    u.short_one
                } else {
                    u.short_other
                }
            }
            (None, _) => "{0}",
        };
        let (pre, post) = pat.split_once("{0}").unwrap_or(("", ""));
        let mut g = Vec::new();
        let mut gu = Vec::new();
        let us = String::from(singular(UNITS[i]));
        if !pre.is_empty() {
            g.push((String::from("literal"), String::from(pre)));
            gu.push(us.clone());
        }
        for p in np {
            g.push(p);
            gu.push(us.clone());
        }
        if !post.is_empty() {
            g.push((String::from("literal"), String::from(post)));
            gu.push(us.clone());
        }
        groups.push((g, gu));
        i += 1;
    }
    // ListFormat type "unit": ", " for long / short / digital, " " for narrow.
    let sep = if f.style == "narrow" { " " } else { ", " };
    let mut out = Vec::new();
    let mut units = Vec::new();
    for (k, (g, gu)) in groups.into_iter().enumerate() {
        if k > 0 {
            out.push((String::from("literal"), String::from(sep)));
            units.push(String::new());
        }
        out.extend(g);
        units.extend(gu);
    }
    (out, units)
}

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "DurationFormat", 0, df_ctor);
    method(vm, p, "format", 1, df_format);
    method(vm, p, "formatToParts", 1, df_format_to_parts);
    method(vm, p, "resolvedOptions", 0, df_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_duration_format_proto = p;
}

fn df_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "DurationFormat")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_duration_format_proto)?;
    let l = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let f = create(vm, &l, &o)?;
    Ok(Value::Object(alloc_intl(vm, p, IntlData::DurationFormat(Box::new(f)))))
}

fn df_args(vm: &mut Vm, ctx: &CallCtx, what: &str) -> JsResult<(DurFmt, [f64; 10])> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::DurationFormat(_)), what)?;
    let v = vm.arg(ctx, 0);
    let rec = to_record(vm, &v)?;
    let f = match intl_ref(vm, o) {
        IntlData::DurationFormat(f) => (**f).clone(),
        _ => unreachable!(),
    };
    Ok((f, rec))
}

fn df_format(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (f, rec) = df_args(vm, ctx, "Intl.DurationFormat.prototype.format")?;
    let (ps, _) = parts(&f, &rec);
    Ok(Value::str(&concat_parts(&ps)))
}

fn df_format_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (f, rec) = df_args(vm, ctx, "Intl.DurationFormat.prototype.formatToParts")?;
    let (ps, units) = parts(&f, &rec);
    Ok(parts_array(vm, &ps, Some(("unit", &units))))
}

fn df_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::DurationFormat(_)), "Intl.DurationFormat.prototype.resolvedOptions")?;
    let f = match intl_ref(vm, o) {
        IntlData::DurationFormat(f) => (**f).clone(),
        _ => unreachable!(),
    };
    let mut props: Vec<(&str, Value)> =
        vec![("locale", Value::str(&f.locale)), ("numberingSystem", Value::str("latn")), ("style", Value::str(f.style))];
    let names: Vec<(String, String)> = UNITS.iter().map(|u| (String::from(*u), format!("{}Display", singular(u)))).collect();
    let mut owned: Vec<(String, Value)> = Vec::new();
    for (i, (u, d)) in names.iter().enumerate() {
        let st = if f.units[i].0 == "fractional" { "numeric" } else { f.units[i].0 };
        owned.push((u.clone(), Value::str(st)));
        owned.push((d.clone(), Value::str(f.units[i].1)));
    }
    let r = object_from(vm, core::mem::take(&mut props));
    for (k, v) in owned {
        vm.heap.get_mut(r).props.insert(PropertyKey::from_str(&k), Prop::data(v, WEC));
    }
    if let Some(fd) = f.fractional_digits {
        vm.heap.get_mut(r).props.insert(PropertyKey::from_str("fractionalDigits"), Prop::data(Value::Number(fd as f64), WEC));
    }
    Ok(Value::Object(r))
}
