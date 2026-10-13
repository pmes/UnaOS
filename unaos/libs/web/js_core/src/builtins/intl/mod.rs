//! ECMA-402 (the ECMAScript Internationalization API, 12th edition): the `Intl` namespace with
//! NumberFormat (§15), DateTimeFormat (§11), Collator (§10), PluralRules (§16), ListFormat (§13),
//! RelativeTimeFormat (§17), Segmenter (§18), Locale (§14), DisplayNames (§12) and DurationFormat (2025), plus the locale-sensitive methods of
//! Number, BigInt, Date, String and Array (§19) that route through them.
//!
//! ICU-free: the locale data is in this tree (`data.rs`), written from CLDR 46's `en`, `en-US` and
//! `en-GB` values (the formats a page reads back from Chromium for those locales). A requested locale
//! outside that set resolves to the default locale `en-US` (ECMA-402 §9.2.7 LookupMatchingLocale); the
//! services report the locale they actually used in `resolvedOptions().locale`, as the spec requires.

use super::*;
use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec;

pub mod collator;
pub mod data;
pub mod datetime;
pub mod displaynames;
pub mod duration;
pub mod list;
pub mod locale;
pub mod number;
pub mod plural;
pub mod relative;
pub mod segmenter;

/// The internal slots of every Intl object (one heap kind, branded by variant).
pub struct IntlObj {
    pub data: IntlData,
    /// The cached bound `format` / `compare` function (§15.3.3, §11.3.3, §10.3.3).
    pub bound: Option<Obj>,
}

pub enum IntlData {
    NumberFormat(Box<number::NumFmt>),
    DateTimeFormat(Box<datetime::DtFmt>),
    Collator(Box<collator::Coll>),
    PluralRules(Box<plural::Plural>),
    ListFormat(Box<list::ListFmt>),
    RelativeTimeFormat(Box<relative::RelFmt>),
    Segmenter(segmenter::Seg),
    Segments { seg: segmenter::Seg, s: JsStr },
    SegmentIterator { seg: segmenter::Seg, s: JsStr, pos: usize },
    Locale(Box<locale::LangTag>),
    DurationFormat(Box<duration::DurFmt>),
    DisplayNames(Box<displaynames::DnFmt>),
    /// The internal slots of a Temporal object (TEMPORAL SR117) — the Intl heap kind carries them so the
    /// engine's object model needs no new variant.
    Temporal(Box<crate::builtins::temporal::TData>),
}

pub fn alloc_intl(vm: &mut Vm, proto: Obj, data: IntlData) -> Obj {
    vm.alloc(ObjectData::new(Some(proto), Kind::Intl(Box::new(IntlObj { data, bound: None }))))
}

/// The locales this implementation has data for (ECMA-402 §9.1 [[AvailableLocales]]).
pub const AVAILABLE: &[&str] = &["en", "en-GB", "en-US"];
/// DefaultLocale (§6.2.4).
pub const DEFAULT_LOCALE: &str = "en-US";

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let intl = vm.new_object(Some(op));
    to_str_tag(vm, intl, "Intl");
    method(vm, intl, "getCanonicalLocales", 1, get_canonical_locales);
    method(vm, intl, "supportedValuesOf", 1, supported_values_of);
    number::init(vm, intl);
    datetime::init(vm, intl);
    collator::init(vm, intl);
    plural::init(vm, intl);
    list::init(vm, intl);
    relative::init(vm, intl);
    segmenter::init(vm, intl);
    locale::init(vm, intl);
    duration::init(vm, intl);
    displaynames::init(vm, intl);
    global(vm, "Intl", Value::Object(intl));
}

// ============================================================================================ helpers

/// Install a service constructor on `intl` with `supportedLocalesOf` and a `[Symbol.toStringTag]`.
pub fn service(vm: &mut Vm, intl: Obj, name: &str, len: u32, f: NativeFn) -> (Obj, Obj) {
    let op = vm.intr().object_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, name, len, f, proto);
    method(vm, c, "supportedLocalesOf", 1, supported_locales_of);
    to_str_tag(vm, proto, &format!("Intl.{name}"));
    value(vm, intl, name, Value::Object(c), WC);
    (c, proto)
}

pub fn require_new(vm: &mut Vm, ctx: &CallCtx, name: &str) -> JsResult<()> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type(&format!("Constructor Intl.{name} requires 'new'"));
    }
    Ok(())
}

/// The Intl object behind `v`, or a TypeError naming the method.
pub fn this_intl(vm: &mut Vm, v: &Value, pick: fn(&IntlData) -> bool, what: &str) -> JsResult<Obj> {
    if let Value::Object(o) = v {
        if let Kind::Intl(i) = &vm.heap.get(*o).kind {
            if pick(&i.data) {
                return Ok(*o);
            }
        }
    }
    vm.throw_type(&format!("Method {what} called on incompatible receiver"))
}

pub fn intl_ref(vm: &Vm, o: Obj) -> &IntlData {
    match &vm.heap.get(o).kind {
        Kind::Intl(i) => &i.data,
        _ => unreachable!("intl brand checked"),
    }
}

/// A plain object with data properties in order (resolvedOptions, parts).
pub fn object_from(vm: &mut Vm, props: Vec<(&str, Value)>) -> Obj {
    let o = vm.new_plain_object();
    for (k, v) in props {
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str(k), Prop::data(v, WEC));
    }
    o
}

/// An array of `{type, value}` part objects (plus an optional extra key such as `source` or `unit`).
pub fn parts_array(vm: &mut Vm, parts: &[(String, String)], extra: Option<(&str, &[String])>) -> Value {
    let mut out = Vec::with_capacity(parts.len());
    for (i, (t, v)) in parts.iter().enumerate() {
        let mut props = vec![("type", Value::str(t)), ("value", Value::str(v))];
        if let Some((k, vals)) = extra {
            if let Some(x) = vals.get(i) {
                if !x.is_empty() {
                    props.push((k, Value::str(x)));
                }
            }
        }
        let o = object_from(vm, props);
        out.push(Value::Object(o));
    }
    Value::Object(vm.new_array(out))
}

pub fn concat_parts(parts: &[(String, String)]) -> String {
    let mut s = String::new();
    for (_, v) in parts {
        s.push_str(v);
    }
    s
}

pub fn strs(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

/// CoerceOptionsToObject (§9.2.11): undefined → no options, otherwise ToObject.
pub fn coerce_options(vm: &mut Vm, v: &Value) -> JsResult<Option<Obj>> {
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(vm.to_object(v)?.as_object())
}

/// GetOptionsObject (§9.2.10): undefined → no options, an object → itself, anything else → TypeError.
pub fn options_object(vm: &mut Vm, v: &Value) -> JsResult<Option<Obj>> {
    match v {
        Value::Undefined => Ok(None),
        Value::Object(o) => Ok(Some(*o)),
        _ => vm.throw_type("Options must be an object"),
    }
}

pub fn get_prop(vm: &mut Vm, o: Option<Obj>, name: &str) -> JsResult<Value> {
    match o {
        Some(o) => vm.get(o, &PropertyKey::from_str(name)),
        None => Ok(Value::Undefined),
    }
}

/// GetOption (§9.2.12) for a string-valued option: `None` when absent; RangeError outside `values`.
pub fn get_string_option(vm: &mut Vm, o: Option<Obj>, name: &str, values: &[&str], service: &str) -> JsResult<Option<String>> {
    let v = get_prop(vm, o, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    let s = vm.to_string(&v)?.to_rust();
    if !values.is_empty() && !values.contains(&s.as_str()) {
        return vm.throw_range(&format!("Value {s} out of range for {service} options property {name}"));
    }
    Ok(Some(s))
}

/// GetOption for a boolean option.
pub fn get_bool_option(vm: &mut Vm, o: Option<Obj>, name: &str) -> JsResult<Option<bool>> {
    let v = get_prop(vm, o, name)?;
    if v.is_undefined() {
        return Ok(None);
    }
    Ok(Some(vm.to_boolean(&v)))
}

/// DefaultNumberOption (§9.2.15).
pub fn default_number_option(vm: &mut Vm, v: &Value, min: f64, max: f64, name: &str) -> JsResult<Option<u32>> {
    if v.is_undefined() {
        return Ok(None);
    }
    let n = vm.to_number(v)?;
    if n.is_nan() || n < min || n > max {
        return vm.throw_range(&format!("{name} value is out of range."));
    }
    Ok(Some(crate::numconv::libm_floor(n) as u32))
}

/// GetNumberOption (§9.2.14).
pub fn get_number_option(vm: &mut Vm, o: Option<Obj>, name: &str, min: f64, max: f64) -> JsResult<Option<u32>> {
    let v = get_prop(vm, o, name)?;
    default_number_option(vm, &v, min, max, name)
}

// ============================================================================================ locales

/// CanonicalizeLocaleList (§9.2.1).
pub fn canonicalize_locale_list(vm: &mut Vm, locales: &Value) -> JsResult<Vec<String>> {
    let mut seen: Vec<String> = Vec::new();
    if locales.is_undefined() {
        return Ok(seen);
    }
    let items: Vec<Value> = match locales {
        Value::String(_) => vec![locales.clone()],
        Value::Object(o) if matches!(&vm.heap.get(*o).kind, Kind::Intl(i) if matches!(i.data, IntlData::Locale(_))) => vec![locales.clone()],
        _ => {
            let o = vm.to_object(locales)?.as_object().unwrap();
            let len = vm.length_of(o)?;
            let mut v = Vec::new();
            let mut k = 0.0;
            while k < len {
                vm.tick()?;
                let key = PropertyKey::from_f64(k);
                if vm.has_property(o, &key)? {
                    v.push(vm.get(o, &key)?);
                }
                k += 1.0;
            }
            v
        }
    };
    for it in items {
        let tag = match &it {
            Value::String(s) => s.to_rust(),
            Value::Object(o) => {
                if let Kind::Intl(i) = &vm.heap.get(*o).kind {
                    if let IntlData::Locale(l) = &i.data {
                        let t = l.to_string();
                        if !seen.contains(&t) {
                            seen.push(t);
                        }
                        continue;
                    }
                }
                vm.to_string(&it)?.to_rust()
            }
            _ => return vm.throw_type("Language ID should be string or object."),
        };
        match locale::LangTag::parse(&tag) {
            Some(t) => {
                let c = t.to_string();
                if !seen.contains(&c) {
                    seen.push(c);
                }
            }
            None => return vm.throw_range(&format!("Incorrect locale information provided: {tag}")),
        }
    }
    Ok(seen)
}

/// BestAvailableLocale (§9.2.2): the longest available prefix of `tag` (extensions removed).
pub fn best_available(tag: &str) -> Option<&'static str> {
    let mut cand: String = locale::strip_extensions(tag);
    loop {
        if let Some(a) = AVAILABLE.iter().find(|a| a.eq_ignore_ascii_case(&cand)) {
            return Some(a);
        }
        let pos = cand.rfind('-')?;
        cand.truncate(pos);
        if cand.len() >= 2 && cand.as_bytes()[cand.len() - 2] == b'-' {
            cand.truncate(cand.len() - 2);
        }
    }
}

/// The outcome of ResolveLocale (§9.2.7): the data locale and the Unicode extension keywords of the
/// request that the service honours (key, value).
pub struct Resolved {
    pub data_locale: &'static str,
    pub keywords: Vec<(String, String)>,
}

/// ResolveLocale with the lookup matcher (best fit is implementation-defined; lookup is a valid best fit).
pub fn resolve_locale(requested: &[String], relevant_keys: &[&str]) -> Resolved {
    for r in requested {
        if let Some(a) = best_available(r) {
            let mut keywords = Vec::new();
            if let Some(t) = locale::LangTag::parse(r) {
                for (k, v) in &t.keywords {
                    if relevant_keys.contains(&k.as_str()) {
                        keywords.push((k.clone(), v.clone()));
                    }
                }
            }
            return Resolved { data_locale: a, keywords };
        }
    }
    Resolved { data_locale: DEFAULT_LOCALE, keywords: Vec::new() }
}

impl Resolved {
    pub fn keyword(&self, k: &str) -> Option<&str> {
        self.keywords.iter().find(|(a, _)| a == k).map(|(_, v)| v.as_str())
    }
    /// The resolved locale string: the data locale plus the honoured keywords (in key order).
    pub fn locale_with(&self, kept: &[(&str, &str)]) -> String {
        let mut s = String::from(self.data_locale);
        if !kept.is_empty() {
            let mut ks: Vec<&(&str, &str)> = kept.iter().collect();
            ks.sort_by(|a, b| a.0.cmp(b.0));
            s.push_str("-u");
            for (k, v) in ks {
                s.push('-');
                s.push_str(k);
                if !v.is_empty() && *v != "true" {
                    s.push('-');
                    s.push_str(v);
                }
            }
        }
        s
    }
}

/// The en-GB data set or the en-US one (`en` resolves to en-US data).
pub fn is_gb(locale: &str) -> bool {
    locale.starts_with("en-GB")
}

fn get_canonical_locales(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let l = vm.arg(ctx, 0);
    let list = canonicalize_locale_list(vm, &l)?;
    let vals = list.iter().map(|s| Value::str(s)).collect();
    Ok(Value::Object(vm.new_array(vals)))
}

/// SupportedLocales (§9.2.10) shared by every service's `supportedLocalesOf`.
fn supported_locales_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let l = vm.arg(ctx, 0);
    let list = canonicalize_locale_list(vm, &l)?;
    let ov = vm.arg(ctx, 1);
    let o = coerce_options(vm, &ov)?;
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], "Intl")?;
    let vals = list.iter().filter(|t| best_available(t).is_some()).map(|s| Value::str(s)).collect();
    Ok(Value::Object(vm.new_array(vals)))
}

/// Intl.supportedValuesOf (§8.3.2).
fn supported_values_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let kv = vm.arg(ctx, 0);
    let key = vm.to_string(&kv)?.to_rust();
    let mut list: Vec<String> = match key.as_str() {
        "calendar" => strs(&["gregory"]),
        // The collator implements the root order only (no tailorings such as "emoji" or "phonebk").
        "collation" => Vec::new(),
        "currency" => data::CURRENCIES.iter().map(|c| c.code.to_string()).collect(),
        "numberingSystem" => strs(&["latn"]),
        "timeZone" => data::ZONES.iter().map(|z| z.id.to_string()).chain(core::iter::once(String::from("UTC"))).collect(),
        "unit" => data::UNITS.iter().map(|u| u.id.to_string()).collect(),
        _ => return vm.throw_range(&format!("Invalid key : {key}")),
    };
    list.sort();
    list.dedup();
    let vals = list.iter().map(|s| Value::str(s)).collect();
    Ok(Value::Object(vm.new_array(vals)))
}

/// The bound-function getter shape shared by NumberFormat/DateTimeFormat `format` and Collator `compare`.
pub fn bound_getter(vm: &mut Vm, this: Obj, name: &str, len: u32, f: NativeFn) -> Value {
    if let Kind::Intl(i) = &vm.heap.get(this).kind {
        if let Some(b) = i.bound {
            return Value::Object(b);
        }
    }
    let fp = vm.intr().function_proto;
    let b = vm.make_native_with(name, len, f, false, Some(fp), vec![Value::Object(this)]);
    if let Kind::Intl(i) = &mut vm.heap.get_mut(this).kind {
        i.bound = Some(b);
    }
    Value::Object(b)
}

/// The Intl object a bound function closes over.
pub fn bound_target(vm: &Vm, ctx: &CallCtx) -> Obj {
    vm.native_slot(ctx.callee, 0).as_object().expect("bound intl function")
}
