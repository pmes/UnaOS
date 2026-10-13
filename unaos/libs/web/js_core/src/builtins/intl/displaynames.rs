//! Intl.DisplayNames (ECMA-402 §12) over the CLDR 46 localeDisplayNames of the locale (INTLCLDR, SR137):
//! languages (with the dialect names CLDR files under language-region / language-script keys and the
//! "-alt-short" forms), regions, scripts, currencies, calendars and the date-time field names, composed with the
//! locale's localePattern / localeSeparator. CLDR's placeholders for unknown codes (und, ZZ, Zzzz, XXX) count as
//! absent. A code the data lacks falls back to the canonicalized code (`fallback: "code"`) or undefined.

use super::locale::LangTag;
use super::*;
use crate::cldr::{slot, L};

#[derive(Clone, Debug)]
pub struct DnFmt {
    pub locale: String,
    pub loc: L,
    pub style: &'static str,
    pub kind: &'static str,
    pub fallback: &'static str,
    pub language_display: &'static str,
}

const UNKNOWN: &[&str] = &["und", "ZZ", "Zzzz", "XXX"];

/// A name of a keyed family, the "-alt-short" form first for the short and narrow styles.
fn named(f: &DnFmt, look: fn(L, &str) -> Option<&'static str>, code: &str) -> Option<String> {
    if UNKNOWN.contains(&code) {
        return None;
    }
    if f.style != "long" {
        if let Some(n) = look(f.loc, &format!("{code}-alt-short")) {
            return Some(String::from(n));
        }
    }
    look(f.loc, code).map(String::from)
}

fn lang_name(l: L, k: &str) -> Option<&'static str> {
    l.language_name(k)
}
fn region_name(l: L, k: &str) -> Option<&'static str> {
    l.region_name(k)
}
fn script_name(l: L, k: &str) -> Option<&'static str> {
    l.script_name(k)
}

/// The language display name (UTS #35 Part 2 §4.1): the longest dialect key the data has (dialect mode), the
/// rest of the tag in the locale's pattern "{0} ({1})" joined by its separator.
fn language_name(f: &DnFmt, t: &LangTag, dialect: bool) -> Option<String> {
    let lang = t.language.as_str();
    let mut used_script = false;
    let mut used_region = false;
    let mut name: Option<String> = None;
    if dialect {
        let mut tries: Vec<(String, bool, bool)> = Vec::new();
        if let (Some(sc), Some(r)) = (&t.script, &t.region) {
            tries.push((format!("{lang}-{sc}-{r}"), true, true));
        }
        if let Some(r) = &t.region {
            tries.push((format!("{lang}-{r}"), false, true));
        }
        if let Some(sc) = &t.script {
            tries.push((format!("{lang}-{sc}"), true, false));
        }
        for (k, us, ur) in tries {
            if let Some(n) = named(f, lang_name, &k) {
                name = Some(n);
                used_script = us;
                used_region = ur;
                break;
            }
        }
    }
    let name = match name {
        Some(n) => n,
        None => named(f, lang_name, lang)?,
    };
    let mut extra: Vec<String> = Vec::new();
    if let Some(sc) = &t.script {
        if !used_script {
            extra.push(named(f, script_name, sc).unwrap_or_else(|| sc.clone()));
        }
    }
    if let Some(r) = &t.region {
        if !used_region {
            extra.push(f.loc.region_name(r).map(String::from).unwrap_or_else(|| r.clone()));
        }
    }
    for v in &t.variants {
        extra.push(v.to_ascii_uppercase());
    }
    if extra.is_empty() {
        return Some(name);
    }
    let sep = f.loc.get(slot::LDP_LOCALESEPARATOR).unwrap_or("{0}, {1}");
    let mut joined = extra[0].clone();
    for e in &extra[1..] {
        joined = sep.replace("{0}", &joined).replace("{1}", e);
    }
    let pat = f.loc.get(slot::LDP_LOCALEPATTERN).unwrap_or("{0} ({1})");
    Some(pat.replace("{0}", &name).replace("{1}", &joined))
}

/// The display name of a date-time field (CLDR dateFields displayName) in a width.
fn field_name(f: &DnFmt, code: &str) -> Option<String> {
    let w = match f.style {
        "short" => 1,
        "narrow" => 2,
        _ => 0,
    };
    let rel = |name: &str| crate::cldr::FIELDS.iter().position(|x| *x == name).and_then(|i| f.loc.relative(i, w, 17));
    let extra = |i: usize| f.loc.get(slot::REL_ERA_NAME + i * 3 + w).or_else(|| f.loc.get(slot::REL_ERA_NAME + i * 3));
    let n = match code {
        "era" => extra(0),
        "weekday" => extra(1),
        "dayPeriod" => extra(2),
        "timeZoneName" => extra(3),
        "weekOfYear" => rel("week"),
        "year" | "quarter" | "month" | "day" | "hour" | "minute" | "second" => rel(code),
        _ => None,
    };
    n.map(String::from)
}

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "DisplayNames", 2, dn_ctor);
    method(vm, p, "of", 1, dn_of);
    method(vm, p, "resolvedOptions", 0, dn_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_display_names_proto = p;
}

fn dn_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "DisplayNames")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_display_names_proto)?;
    let l = vm.arg(ctx, 0);
    let requested = canonicalize_locale_list(vm, &l)?;
    let ov = vm.arg(ctx, 1);
    let o = match ov {
        Value::Object(o) => Some(o),
        _ => return vm.throw_type("Intl.DisplayNames requires an options object with a type"),
    };
    let svc = "Intl.DisplayNames";
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], svc)?;
    let r = resolve_locale(&requested, &[]);
    let style = get_string_option(vm, o, "style", &["narrow", "short", "long"], svc)?;
    let kind = get_string_option(vm, o, "type", &["language", "region", "script", "currency", "calendar", "dateTimeField"], svc)?;
    let Some(kind) = kind else { return vm.throw_type("Intl.DisplayNames: the type option is required") };
    let fallback = get_string_option(vm, o, "fallback", &["code", "none"], svc)?;
    let ld = get_string_option(vm, o, "languageDisplay", &["dialect", "standard"], svc)?;
    let f = DnFmt {
        locale: String::from(r.data_locale),
        loc: r.loc,
        style: match style.as_deref() {
            Some("narrow") => "narrow",
            Some("short") => "short",
            _ => "long",
        },
        kind: match kind.as_str() {
            "language" => "language",
            "region" => "region",
            "script" => "script",
            "currency" => "currency",
            "calendar" => "calendar",
            _ => "dateTimeField",
        },
        fallback: if fallback.as_deref() == Some("none") { "none" } else { "code" },
        language_display: if ld.as_deref() == Some("standard") { "standard" } else { "dialect" },
    };
    Ok(Value::Object(alloc_intl(vm, p, IntlData::DisplayNames(Box::new(f)))))
}

fn dn_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::DisplayNames(_)), "Intl.DisplayNames.prototype.of")?;
    let f = match intl_ref(vm, o) {
        IntlData::DisplayNames(f) => (**f).clone(),
        _ => unreachable!(),
    };
    let cv = vm.arg(ctx, 0);
    let code = vm.to_string(&cv)?.to_rust();
    let bad = |vm: &mut Vm| -> JsResult<Value> { vm.throw_range(&format!("invalid_argument: {code}")) };
    let (canon, name): (String, Option<String>) = match f.kind {
        "language" => {
            let t = match LangTag::parse(&code) {
                Some(t) if t.keywords.is_empty() && t.attributes.is_empty() && t.other.is_empty() => t,
                _ => return bad(vm),
            };
            (t.to_string(), language_name(&f, &t, f.language_display == "dialect"))
        }
        "region" => {
            let ok = (code.len() == 2 && code.bytes().all(|b| b.is_ascii_alphabetic())) || (code.len() == 3 && code.bytes().all(|b| b.is_ascii_digit()));
            if !ok {
                return bad(vm);
            }
            let c = code.to_ascii_uppercase();
            let n = named(&f, region_name, &c);
            (c, n)
        }
        "script" => {
            if !(code.len() == 4 && code.bytes().all(|b| b.is_ascii_alphabetic())) {
                return bad(vm);
            }
            let l = code.to_ascii_lowercase();
            let c = format!("{}{}", l[..1].to_ascii_uppercase(), &l[1..]);
            let n = named(&f, script_name, &c);
            (c, n)
        }
        "currency" => {
            if !(code.len() == 3 && code.bytes().all(|b| b.is_ascii_alphabetic())) {
                return bad(vm);
            }
            let c = code.to_ascii_uppercase();
            let n = if UNKNOWN.contains(&c.as_str()) { None } else { f.loc.currency(&c, 0).map(String::from) };
            (c, n)
        }
        "calendar" => {
            if !code.split('-').all(|p| (3..=8).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_alphanumeric())) {
                return bad(vm);
            }
            let c = code.to_ascii_lowercase();
            let c = if c == "gregorian" { String::from("gregory") } else { c };
            let key = match c.as_str() {
                "gregory" => "gregorian",
                "ethioaa" => "ethiopic-amete-alem",
                "islamicc" => "islamic-civil",
                other => other,
            };
            let n = f.loc.calendar_name(key).map(String::from);
            (c, n)
        }
        _ => {
            const FIELD_CODES: [&str; 12] =
                ["era", "year", "quarter", "month", "weekOfYear", "weekday", "day", "dayPeriod", "hour", "minute", "second", "timeZoneName"];
            if !FIELD_CODES.contains(&code.as_str()) {
                return bad(vm);
            }
            let n = field_name(&f, &code);
            (code.clone(), n)
        }
    };
    Ok(match name {
        Some(n) => Value::str(&n),
        None if f.fallback == "code" => Value::str(&canon),
        None => Value::Undefined,
    })
}

fn dn_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, |d| matches!(d, IntlData::DisplayNames(_)), "Intl.DisplayNames.prototype.resolvedOptions")?;
    let f = match intl_ref(vm, o) {
        IntlData::DisplayNames(f) => (**f).clone(),
        _ => unreachable!(),
    };
    let mut props = vec![
        ("locale", Value::str(&f.locale)),
        ("style", Value::str(f.style)),
        ("type", Value::str(f.kind)),
        ("fallback", Value::str(f.fallback)),
    ];
    if f.kind == "language" {
        props.push(("languageDisplay", Value::str(f.language_display)));
    }
    let r = object_from(vm, props);
    Ok(Value::Object(r))
}
