//! Intl.Collator (ECMA-402 §10) and String.prototype.localeCompare (§19.1.1): the Unicode Collation Algorithm
//! (UTS #10) over CLDR 46's root collation (allkeys_CLDR.txt, the DUCET with CLDR's changes) and the CLDR
//! tailorings resolved by tools/gen_cldr.py (INTLCLDR, SR137): de-u-co-phonebk, es (standard) and
//! es-u-co-trad, sv, da (caseFirst upper), tr, ja (kana rules and the JIS kanji order) and zh (pinyin).
//!
//! Both strings are normalized to NFD, mapped to collation elements by the longest match over the tailoring
//! and then the root (contractions included), code points without an entry take the UCA implicit weights,
//! and the elements compare level by level: primary, secondary (accents), tertiary (case and variants; the
//! case bits alone for `sensitivity: "case"`, upper before lower under caseFirst upper). `numeric` replaces a
//! digit run by its length and digits; `ignorePunctuation` drops the variable-weighted elements. Refused by
//! name: zh stroke / zhuyin / unihan, ja unihan, the search collations, and the tailorings of the other
//! locales here (pl, uk, vi, ko, ar, he, hi, th, fa, bn …), which collate with the root order.

use super::*;
use crate::cldr::collation as ct;
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
    /// Index into `cldr::collation::TAILORINGS` (None: the root order).
    pub tailoring: Option<usize>,
    /// The collation type resolvedOptions reports ("default", "phonebk", "trad", "pinyin").
    pub collation: &'static str,
}

/// A collation element in the widened weights of the generated tables.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Ce {
    p: u32,
    s: u16,
    t: u16,
    v: bool,
}

fn widen_root(x: u32) -> Ce {
    Ce { p: (x >> 16) << 16, s: (((x >> 5) & 0x3ff) << 6) as u16, t: ((x & 0x1f) << 4) as u16, v: (x >> 15) & 1 == 1 }
}

fn unwide(x: u64) -> Ce {
    Ce { p: (x >> 32) as u32, s: (x >> 16) as u16, t: ((x >> 1) & 0x7fff) as u16, v: x & 1 == 1 }
}

fn root_single(cp: u32) -> Option<&'static [u32]> {
    let i = ct::ROOT_CP.binary_search(&cp).ok()?;
    let block = i / 64;
    let mut start = ct::ROOT_OFF[block] as usize;
    for k in block * 64..i {
        start += ct::ROOT_N[k] as usize;
    }
    Some(&ct::ROOT_CES[start..start + ct::ROOT_N[i] as usize])
}

/// UCA §10.1.3 implicit weights (widened): two elements.
fn implicit(cp: u32, out: &mut Vec<Ce>) {
    let ui = unicode::binary_property("Unified_Ideograph").map(|t| unicode::in_table(t, cp)).unwrap_or(false);
    let (a, b) = if (0x17000..=0x18AFF).contains(&cp) || (0x18D00..=0x18D8F).contains(&cp) {
        (0xFB00, (cp - 0x17000) | 0x8000)
    } else if (0x1B170..=0x1B2FF).contains(&cp) {
        (0xFB01, (cp - 0x1B170) | 0x8000)
    } else if (0x18B00..=0x18CFF).contains(&cp) {
        (0xFB02, (cp - 0x18B00) | 0x8000)
    } else if ui && ((0x4E00..=0x9FFF).contains(&cp) || (0xF900..=0xFAFF).contains(&cp)) {
        (0xFB40 + (cp >> 15), (cp & 0x7FFF) | 0x8000)
    } else if ui {
        (0xFB80 + (cp >> 15), (cp & 0x7FFF) | 0x8000)
    } else {
        (0xFBC0 + (cp >> 15), (cp & 0x7FFF) | 0x8000)
    };
    out.push(Ce { p: a << 16, s: 0x20 << 6, t: 0x02 << 4, v: false });
    out.push(Ce { p: b << 16, s: 0, t: 0, v: false });
}

/// Whether some key of a sorted multi-code-point table starts with `prefix`.
fn has_prefix<T>(keys: &[T], key: impl Fn(&T) -> &str, prefix: &str) -> bool {
    let i = keys.partition_point(|k| key(k) < prefix);
    i < keys.len() && key(&keys[i]).starts_with(prefix)
}

/// Append the elements of the longest match at `cps[i..]`; returns the code points consumed.
fn lookup(tl: Option<&ct::Tailoring>, cps: &[u32], i: usize, out: &mut Vec<Ce>) -> usize {
    let cp = cps[i];
    let mut first = String::new();
    first.push(char::from_u32(cp).unwrap_or('\u{fffd}'));
    let in_tl = tl.map(|t| has_prefix(t.multi, |k| k.0, &first)).unwrap_or(false);
    let in_root = has_prefix(ct::ROOT_CONTR, |k| k.0, &first);
    if in_tl || in_root {
        for n in (2..=4usize).rev() {
            if i + n > cps.len() {
                continue;
            }
            let key: String = cps[i..i + n].iter().map(|c| char::from_u32(*c).unwrap_or('\u{fffd}')).collect();
            if let Some(t) = tl {
                if let Ok(k) = t.multi.binary_search_by(|e| e.0.cmp(key.as_str())) {
                    let x = t.multi[k].1;
                    let (start, count) = ((x >> 5) as usize, (x & 31) as usize);
                    out.extend(t.ces[start..start + count].iter().map(|c| unwide(*c)));
                    return n;
                }
            }
            if let Ok(k) = ct::ROOT_CONTR.binary_search_by(|e| e.0.cmp(key.as_str())) {
                let (_, start, count) = ct::ROOT_CONTR[k];
                out.extend(ct::ROOT_CES[start as usize..start as usize + count as usize].iter().map(|c| widen_root(*c)));
                return n;
            }
        }
    }
    if let Some(t) = tl {
        if let Ok(k) = t.single.binary_search(&cp) {
            let x = t.single_ce[k];
            if x & 0x8000_0000 != 0 {
                let mut c = unwide(t.anchors[((x >> 16) & 0x7fff) as usize]);
                c.p += x & 0xffff;
                out.push(c);
            } else {
                let (start, count) = ((x >> 5) as usize, (x & 31) as usize);
                out.extend(t.ces[start..start + count].iter().map(|c| unwide(*c)));
            }
            return 1;
        }
    }
    match root_single(cp) {
        Some(ces) => out.extend(ces.iter().map(|c| widen_root(*c))),
        None => implicit(cp, out),
    }
    1
}

fn is_digit_gc(gc: u32) -> bool {
    gc == 13
}

/// The collation elements of `s` (UTF-16) after NFD.
fn elements(c: &Coll, s: &[u16]) -> Vec<Ce> {
    let mut cps = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let (cp, n) = code_point_at(s, i);
        cps.push(cp);
        i += n;
    }
    let cps = unicode::normalize(&cps, 1);
    let tl = c.tailoring.map(|k| &ct::TAILORINGS[k]);
    let mut out: Vec<Ce> = Vec::with_capacity(cps.len() + 4);
    let mut i = 0;
    while i < cps.len() {
        let cp = cps[i];
        if c.numeric && is_digit_gc(unicode::general_category(cp)) {
            let mut j = i;
            let mut digits: Vec<u32> = Vec::new();
            while j < cps.len() && is_digit_gc(unicode::general_category(cps[j])) {
                digits.push(digit_value(cps[j]));
                j += 1;
            }
            while digits.len() > 1 && digits[0] == 0 {
                digits.remove(0);
            }
            let zero = root_single(0x30).and_then(|x| x.first().copied()).map(widen_root).unwrap_or(Ce { p: 0, s: 0, t: 0, v: false });
            let len = (digits.len() as u32).min(0x7fff);
            out.push(Ce { p: zero.p - 0x8000 + len, ..zero });
            for d in digits {
                if let Some(x) = root_single(0x30 + d).and_then(|x| x.first().copied()) {
                    out.push(widen_root(x));
                }
            }
            i = j;
            continue;
        }
        i += lookup(tl, &cps, i, &mut out);
    }
    if c.ignore_punctuation {
        out.retain(|e| !e.v);
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

/// The root tertiary weights of uppercase forms (UCA tertiary table: capital, wide / compat / font / circled
/// capital).
fn is_upper(t: u16) -> bool {
    matches!(t >> 4, 0x08..=0x0C | 0x1D)
}

/// Compare two strings under the collator (§10.3.3.1 CompareStrings).
pub fn compare(c: &Coll, a: &[u16], b: &[u16]) -> Ordering {
    let ea = elements(c, a);
    let eb = elements(c, b);
    let prim = |e: &[Ce]| e.iter().filter(|x| x.p != 0).map(|x| x.p).collect::<Vec<_>>();
    let o = prim(&ea).cmp(&prim(&eb));
    if o != Ordering::Equal {
        return o;
    }
    if matches!(c.sensitivity, "accent" | "variant") {
        let sec = |e: &[Ce]| e.iter().filter(|x| x.s != 0).map(|x| x.s).collect::<Vec<_>>();
        let o = sec(&ea).cmp(&sec(&eb));
        if o != Ordering::Equal {
            return o;
        }
    }
    let upper_first = c.case_first == "upper";
    let key = |t: u16| -> u32 {
        if upper_first && is_upper(t) { t as u32 } else if upper_first { t as u32 + 0x400 } else { t as u32 }
    };
    if c.sensitivity == "case" {
        let case = |e: &[Ce]| e.iter().filter(|x| x.p != 0).map(|x| key(if is_upper(x.t) { 0x08 << 4 } else { 0x02 << 4 })).collect::<Vec<_>>();
        let o = case(&ea).cmp(&case(&eb));
        if o != Ordering::Equal {
            return o;
        }
    }
    if c.sensitivity == "variant" {
        let ter = |e: &[Ce]| e.iter().filter(|x| x.t != 0).map(|x| key(x.t)).collect::<Vec<_>>();
        let o = ter(&ea).cmp(&ter(&eb));
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// The tailoring and collation type of a locale with an optional requested type (-u-co- / collation).
fn tailoring_of(lang: &str, tag: &str, co: Option<&str>) -> (Option<usize>, &'static str) {
    let find = |id: &str| ct::TAILORINGS.iter().position(|t| t.id == id);
    let hant = tag.contains("Hant") || tag == "zh-TW" || tag == "zh-HK";
    match (lang, co) {
        ("de", Some("phonebk")) => (find("de-phonebk"), "phonebk"),
        ("es", Some("trad")) => (find("es-trad"), "trad"),
        ("es", _) => (find("es"), "default"),
        ("sv", _) => (find("sv"), "default"),
        ("da", _) => (find("da"), "default"),
        ("tr", _) => (find("tr"), "default"),
        ("ja", _) => (find("ja"), "default"),
        ("zh", Some("pinyin")) if hant => (find("zh"), "pinyin"),
        ("zh", _) if !hant => (find("zh"), "default"),
        _ => (None, "default"),
    }
}

/// The collation types a locale supports besides its default.
fn supported_co(lang: &str, tag: &str) -> &'static [&'static str] {
    match lang {
        "de" => &["phonebk"],
        "es" => &["trad"],
        "zh" if tag.contains("Hant") || tag == "zh-TW" || tag == "zh-HK" => &["pinyin"],
        _ => &[],
    }
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
    let lang = r.loc.lang();
    let tag = r.data_locale;
    let sup = supported_co(lang, tag);
    let kw_co = r.keyword("co").and_then(|v| sup.iter().copied().find(|x| *x == v));
    let opt_co = collation.as_deref().and_then(|v| sup.iter().copied().find(|x| *x == v));
    let (tailoring, coll_type) = tailoring_of(lang, tag, opt_co.or(kw_co));
    let tailored_upper = tailoring.map(|k| ct::TAILORINGS[k].case_upper).unwrap_or(false);
    let numeric_v = numeric.or(kw_kn).unwrap_or(false);
    let case_first_v: &'static str = match case_first.as_deref() {
        Some("upper") => "upper",
        Some("lower") => "lower",
        Some(_) => "false",
        None => kw_kf.unwrap_or(if tailored_upper { "upper" } else { "false" }),
    };
    let mut kept: Vec<(&str, &str)> = Vec::new();
    if let Some(v) = kw_co {
        if opt_co.map(|c| c == v).unwrap_or(true) {
            kept.push(("co", v));
        }
    }
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
        tailoring,
        collation: coll_type,
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
        ("collation", Value::str(c.collation)),
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
