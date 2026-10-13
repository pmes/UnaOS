//! Intl.Collator (ECMA-402 §10) and String.prototype.localeCompare (§19.1.1): a three-level comparison in
//! the shape of the Unicode Collation Algorithm (UTS #10) for the `en` tailoring (= the root order):
//!
//! - level 1 (primary): base characters — the variable group (whitespace, punctuation, symbols) sorts
//!   before digits, digits before letters, Latin letters alphabetically with case and accents folded away,
//!   other letters by their folded code point; combining marks are primary-ignorable;
//! - level 2 (secondary): accents (the combining marks of the canonical decomposition, in order);
//! - level 3 (tertiary): case — lowercase first, or uppercase first under `caseFirst: "upper"`.
//!
//! Canonically equivalent strings compare equal (both sides are compared in NFD). `numeric` compares digit
//! runs by value; `ignorePunctuation` drops the variable group. Without DUCET weights for every script the
//! order between scripts is by code point — documented, not hidden.

use super::*;
use crate::string::code_point_at;
use crate::unicode;
use core::cmp::Ordering;

#[derive(Clone, Debug)]
pub struct Coll {
    pub locale: String,
    pub usage: &'static str,
    pub sensitivity: &'static str,
    pub ignore_punctuation: bool,
    pub numeric: bool,
    pub case_first: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
struct Elem {
    p: u64,
    s: u32,
    t: u8,
}

const G_VARIABLE: u64 = 1 << 40;
const G_DIGIT: u64 = 2 << 40;
const G_LATIN: u64 = 3 << 40;
const G_OTHER: u64 = 4 << 40;

/// Latin letters outside a–z whose NFD keeps no ASCII base: (letter, primary base(s), secondary mark).
fn latin_special(c: u32) -> Option<(&'static str, u32)> {
    Some(match c {
        0xDF => ("ss", 0),
        0x1E9E => ("ss", 0),
        0xE6 | 0xC6 => ("ae", 0),
        0x153 | 0x152 => ("oe", 0),
        0xF8 | 0xD8 => ("o", 0x338),
        0x111 | 0x110 => ("d", 0x335),
        0x142 | 0x141 => ("l", 0x337),
        0x131 => ("i", 0x307),
        0xF0 | 0xD0 => ("d", 0x2FF),
        _ => return None,
    })
}

fn is_mark(gc: u32) -> bool {
    matches!(gc, 10..=12)
}
fn is_letter(gc: u32) -> bool {
    matches!(gc, 5..=9)
}
fn is_digit_gc(gc: u32) -> bool {
    gc == 13
}

/// The collation elements of `s` (UTF-16) after NFD.
fn elements(c: &Coll, s: &[u16]) -> Vec<Elem> {
    let mut cps = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let (cp, n) = code_point_at(s, i);
        cps.push(cp);
        i += n;
    }
    let cps = unicode::normalize(&cps, 1);
    let upper_first = c.case_first == "upper";
    let mut out: Vec<Elem> = Vec::with_capacity(cps.len());
    let mut i = 0;
    while i < cps.len() {
        let cp = cps[i];
        let gc = unicode::general_category(cp);
        if is_mark(gc) {
            out.push(Elem { p: 0, s: cp, t: 0 });
            i += 1;
            continue;
        }
        if gc == 0 || gc == 1 {
            // Controls and format characters are completely ignorable.
            i += 1;
            continue;
        }
        if is_digit_gc(gc) {
            if c.numeric {
                let mut j = i;
                let mut digits: Vec<u32> = Vec::new();
                while j < cps.len() && is_digit_gc(unicode::general_category(cps[j])) {
                    digits.push(digit_value(cps[j]));
                    j += 1;
                }
                while digits.len() > 1 && digits[0] == 0 {
                    digits.remove(0);
                }
                out.push(Elem { p: G_DIGIT | (digits.len() as u64) << 8, s: 0, t: 0 });
                for d in digits {
                    out.push(Elem { p: G_DIGIT | d as u64, s: 0, t: 0 });
                }
                i = j;
                continue;
            }
            out.push(Elem { p: G_DIGIT | digit_value(cp) as u64, s: 0, t: 0 });
            i += 1;
            continue;
        }
        if is_letter(gc) {
            let lower = unicode::simple_lower(cp);
            let is_upper = lower != cp || gc == 9 || gc == 8;
            let t = if is_upper != upper_first { 1 } else { 0 };
            if lower < 0x80 && (lower as u8).is_ascii_lowercase() {
                out.push(Elem { p: G_LATIN | ((lower - 'a' as u32) as u64) << 8, s: 0, t });
            } else if let Some((base, mark)) = latin_special(cp) {
                for b in base.bytes() {
                    out.push(Elem { p: G_LATIN | ((b - b'a') as u64) << 8, s: 0, t });
                }
                if mark != 0 {
                    out.push(Elem { p: 0, s: mark, t: 0 });
                }
            } else if cp == 0xFE || cp == 0xDE {
                // þ sorts after z.
                out.push(Elem { p: G_LATIN | 26 << 8, s: 0, t });
            } else {
                out.push(Elem { p: G_OTHER | unicode::simple_fold(cp) as u64, s: 0, t });
            }
            i += 1;
            continue;
        }
        // The variable group: whitespace, punctuation, symbols.
        if !c.ignore_punctuation {
            out.push(Elem { p: G_VARIABLE | cp as u64, s: 0, t: 0 });
        }
        i += 1;
    }
    out
}

fn digit_value(cp: u32) -> u32 {
    if (0x30..=0x39).contains(&cp) {
        return cp - 0x30;
    }
    // Nd code points come in contiguous runs of ten starting at a zero.
    let mut z = cp;
    while z > 0 && unicode::general_category(z - 1) == 13 && cp - (z - 1) < 10 {
        z -= 1;
    }
    (cp - z) % 10
}

/// Compare two strings under the collator (§10.3.3.1 CompareStrings).
pub fn compare(c: &Coll, a: &[u16], b: &[u16]) -> Ordering {
    let ea = elements(c, a);
    let eb = elements(c, b);
    let prim = |e: &[Elem]| e.iter().filter(|x| x.p != 0).map(|x| x.p).collect::<Vec<_>>();
    let o = prim(&ea).cmp(&prim(&eb));
    if o != Ordering::Equal {
        return o;
    }
    if matches!(c.sensitivity, "accent" | "variant") {
        // Secondary weights: each base carries the marks that follow it.
        let sec = |e: &[Elem]| {
            let mut v: Vec<(usize, u32)> = Vec::new();
            let mut base = 0usize;
            for x in e {
                if x.p != 0 {
                    base += 1;
                } else {
                    v.push((base, x.s));
                }
            }
            v
        };
        let o = sec(&ea).cmp(&sec(&eb));
        if o != Ordering::Equal {
            return o;
        }
    }
    if matches!(c.sensitivity, "case" | "variant") {
        let ter = |e: &[Elem]| e.iter().filter(|x| x.p != 0).map(|x| x.t).collect::<Vec<_>>();
        let o = ter(&ea).cmp(&ter(&eb));
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

pub fn create(vm: &mut Vm, locales: &Value, options: &Value) -> JsResult<Coll> {
    let requested = canonicalize_locale_list(vm, locales)?;
    let o = coerce_options(vm, options)?;
    let svc = "Intl.Collator";
    let usage = get_string_option(vm, o, "usage", &["sort", "search"], svc)?;
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], svc)?;
    let collation = get_string_option(vm, o, "collation", &[], svc)?;
    if let Some(cv) = &collation {
        if !cv.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
            return vm.throw_range(&format!("Invalid collation : {cv}"));
        }
    }
    let numeric = get_bool_option(vm, o, "numeric")?;
    let case_first = get_string_option(vm, o, "caseFirst", &["upper", "lower", "false"], svc)?;
    let r = resolve_locale(&requested, &["co", "kf", "kn"]);
    let sensitivity = get_string_option(vm, o, "sensitivity", &["base", "accent", "case", "variant"], svc)?;
    let ignore = get_bool_option(vm, o, "ignorePunctuation")?;
    let kw_kn = r.keyword("kn").map(|v| v.is_empty() || v == "true");
    let kw_kf = r.keyword("kf").and_then(|v| ["upper", "lower", "false"].into_iter().find(|x| *x == v));
    let numeric_v = numeric.or(kw_kn).unwrap_or(false);
    let case_first_v: &'static str = match case_first.as_deref() {
        Some("upper") => "upper",
        Some("lower") => "lower",
        Some(_) => "false",
        None => kw_kf.unwrap_or("false"),
    };
    let mut kept: Vec<(&str, &str)> = Vec::new();
    if let Some(v) = kw_kf {
        if case_first.as_deref().map(|c| c == v).unwrap_or(true) {
            kept.push(("kf", v));
        }
    }
    if let Some(v) = kw_kn {
        if numeric.map(|n| n == v).unwrap_or(true) {
            kept.push(("kn", if v { "" } else { "false" }));
        }
    }
    let usage_v: &'static str = if usage.as_deref() == Some("search") { "search" } else { "sort" };
    Ok(Coll {
        locale: r.locale_with(&kept),
        usage: usage_v,
        sensitivity: match sensitivity.as_deref() {
            Some("base") => "base",
            Some("accent") => "accent",
            Some("case") => "case",
            _ => "variant",
        },
        ignore_punctuation: ignore.unwrap_or(false),
        numeric: numeric_v,
        case_first: case_first_v,
    })
}

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "Collator", 0, coll_ctor);
    accessor(vm, p, PropertyKey::from_str("compare"), "compare", Some(coll_compare_getter), None, C);
    method(vm, p, "resolvedOptions", 0, coll_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_collator_proto = p;
}

fn coll_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let nt = if ctx.new_target.is_undefined() { Value::Object(ctx.callee) } else { ctx.new_target.clone() };
    let p = vm.get_prototype_from_ctor(&nt, |i| i.intl_collator_proto)?;
    let l = vm.arg(ctx, 0);
    let o = vm.arg(ctx, 1);
    let c = create(vm, &l, &o)?;
    Ok(Value::Object(alloc_intl(vm, p, IntlData::Collator(Box::new(c)))))
}

fn is_coll(d: &IntlData) -> bool {
    matches!(d, IntlData::Collator(_))
}

fn coll_of(vm: &Vm, o: Obj) -> Coll {
    match intl_ref(vm, o) {
        IntlData::Collator(c) => (**c).clone(),
        _ => unreachable!(),
    }
}

fn coll_compare_getter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_coll, "get Intl.Collator.prototype.compare")?;
    Ok(bound_getter(vm, o, "", 2, coll_bound_compare))
}

pub fn ordering_value(o: Ordering) -> Value {
    Value::Number(match o {
        Ordering::Less => -1.0,
        Ordering::Equal => 0.0,
        Ordering::Greater => 1.0,
    })
}

fn coll_bound_compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = bound_target(vm, ctx);
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    let x = vm.to_string(&a)?;
    let y = vm.to_string(&b)?;
    let c = coll_of(vm, o);
    Ok(ordering_value(compare(&c, x.units(), y.units())))
}

fn coll_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_coll, "Intl.Collator.prototype.resolvedOptions")?;
    let c = coll_of(vm, o);
    let props = vec![
        ("locale", Value::str(&c.locale)),
        ("usage", Value::str(c.usage)),
        ("sensitivity", Value::str(c.sensitivity)),
        ("ignorePunctuation", Value::Bool(c.ignore_punctuation)),
        ("collation", Value::str("default")),
        ("numeric", Value::Bool(c.numeric)),
        ("caseFirst", Value::str(c.case_first)),
    ];
    let r = object_from(vm, props);
    Ok(Value::Object(r))
}

/// String.prototype.localeCompare (§19.1.1).
pub fn locale_compare(vm: &mut Vm, s: &JsStr, that: &JsStr, locales: &Value, options: &Value) -> JsResult<Value> {
    let c = create(vm, locales, options)?;
    Ok(ordering_value(compare(&c, s.units(), that.units())))
}
