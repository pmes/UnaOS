//! Intl.ListFormat (ECMA-402 §13) over the CLDR 46 list patterns of the locale (INTLCLDR, SR137): the pair,
//! start, middle and end templates of the standard / or / unit types in long, short and narrow widths, with
//! the contextual forms CLDR leaves to the formatter — Spanish "y" → "e" before an /i/ sound and "o" → "u"
//! before an /o/ sound, Hebrew "ו" → "ו-" before a non-Hebrew word.

use super::*;
use crate::cldr::L;

#[derive(Clone, Debug)]
pub struct ListFmt {
    pub locale: String,
    pub loc: L,
    pub kind: &'static str,
    pub style: &'static str,
}

/// The index of a list type in `cldr::LIST_TYPES`.
pub fn list_type(kind: &str, style: &str) -> usize {
    let base = match kind {
        "disjunction" => 3,
        "unit" => 6,
        _ => 0,
    };
    base + match style {
        "short" => 1,
        "narrow" => 2,
        _ => 0,
    }
}

/// One element of a list: its parts and, per part, an annotation (DurationFormat's unit; "" otherwise).
pub type Elem = (Vec<(String, String)>, Vec<String>);

fn text_of(e: &Elem) -> String {
    let mut s = String::new();
    for (_, v) in &e.0 {
        s.push_str(v);
    }
    s
}

/// The contextual form of a separator before `next` (CLDR's list patterns leave these to the formatter).
fn contextual(lang: &str, sep: &str, next: &str) -> String {
    let lower: String = next.chars().take(4).flat_map(|c| c.to_lowercase()).collect();
    if lang == "es" {
        let i_sound = (lower.starts_with('i') || lower.starts_with("hi"))
            && !(lower.starts_with("hia") || lower.starts_with("hie") || lower.starts_with("hio") || lower.starts_with("hiu"));
        if sep.ends_with(" y ") && i_sound {
            return format!("{} e ", &sep[..sep.len() - 3]);
        }
        let eleven = lower.starts_with("11") && !lower[2..].starts_with(|c: char| c.is_ascii_digit());
        let o_sound = lower.starts_with('o') || lower.starts_with("ho") || lower.starts_with('8') || eleven;
        if sep.ends_with(" o ") && o_sound {
            return format!("{} u ", &sep[..sep.len() - 3]);
        }
    }
    if lang == "he" && sep.ends_with('ו') {
        let hebrew = next.chars().next().map(|c| ('\u{5d0}'..='\u{5ea}').contains(&c)).unwrap_or(false);
        if !hebrew {
            return format!("{sep}-");
        }
    }
    String::from(sep)
}

/// Fill a "{0}…{1}" template with two elements.
fn fill(lang: &str, template: &str, a: Elem, b: Elem) -> Elem {
    let (pre, rest) = template.split_once("{0}").unwrap_or(("", template));
    let (mid, post) = rest.split_once("{1}").unwrap_or((rest, ""));
    let mut parts = Vec::new();
    let mut ann = Vec::new();
    let lit = |s: &str, parts: &mut Vec<(String, String)>, ann: &mut Vec<String>| {
        if !s.is_empty() {
            parts.push((String::from("literal"), String::from(s)));
            ann.push(String::new());
        }
    };
    lit(pre, &mut parts, &mut ann);
    parts.extend(a.0);
    ann.extend(a.1);
    let mid = contextual(lang, mid, &text_of(&b));
    lit(&mid, &mut parts, &mut ann);
    parts.extend(b.0);
    ann.extend(b.1);
    lit(post, &mut parts, &mut ann);
    (parts, ann)
}

/// CreatePartsFromList (§13.5.2) over the templates of list type `ty` of locale `loc`.
pub fn join(loc: L, ty: usize, mut items: Vec<Elem>) -> Elem {
    let lang = loc.lang();
    let n = items.len();
    if n == 0 {
        return (Vec::new(), Vec::new());
    }
    if n == 1 {
        return items.pop().unwrap_or_default();
    }
    if n == 2 {
        let b = items.pop().unwrap_or_default();
        let a = items.pop().unwrap_or_default();
        return fill(lang, loc.list(ty, 3), a, b);
    }
    let mut acc = items.pop().unwrap_or_default();
    let prev = items.pop().unwrap_or_default();
    acc = fill(lang, loc.list(ty, 2), prev, acc);
    while items.len() > 1 {
        let x = items.pop().unwrap_or_default();
        acc = fill(lang, loc.list(ty, 1), x, acc);
    }
    let first = items.pop().unwrap_or_default();
    fill(lang, loc.list(ty, 0), first, acc)
}

/// The parts of a list of strings.
pub fn parts(f: &ListFmt, items: &[String]) -> Vec<(String, String)> {
    let elems: Vec<Elem> = items.iter().map(|s| (vec![(String::from("element"), s.clone())], vec![String::new()])).collect();
    join(f.loc, list_type(f.kind, f.style), elems).0
}

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "ListFormat", 0, lf_ctor);
    method(vm, p, "format", 1, lf_format);
    method(vm, p, "formatToParts", 1, lf_format_to_parts);
    method(vm, p, "resolvedOptions", 0, lf_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_list_format_proto = p;
}

fn lf_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "ListFormat")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_list_format_proto)?;
    let l = vm.arg(ctx, 0);
    let requested = canonicalize_locale_list(vm, &l)?;
    let ov = vm.arg(ctx, 1);
    let o = options_object(vm, &ov)?;
    let svc = "Intl.ListFormat";
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], svc)?;
    let r = resolve_locale(&requested, &[]);
    let kind = get_string_option(vm, o, "type", &["conjunction", "disjunction", "unit"], svc)?;
    let style = get_string_option(vm, o, "style", &["long", "short", "narrow"], svc)?;
    let f = ListFmt {
        locale: String::from(r.data_locale),
        loc: r.loc,
        kind: match kind.as_deref() {
            Some("disjunction") => "disjunction",
            Some("unit") => "unit",
            _ => "conjunction",
        },
        style: match style.as_deref() {
            Some("short") => "short",
            Some("narrow") => "narrow",
            _ => "long",
        },
    };
    Ok(Value::Object(alloc_intl(vm, p, IntlData::ListFormat(Box::new(f)))))
}

fn is_lf(d: &IntlData) -> bool {
    matches!(d, IntlData::ListFormat(_))
}

fn lf_of(vm: &Vm, o: Obj) -> ListFmt {
    match intl_ref(vm, o) {
        IntlData::ListFormat(f) => (**f).clone(),
        _ => unreachable!(),
    }
}

/// StringListFromIterable (§13.5.6): every element must be a String.
fn string_list(vm: &mut Vm, v: &Value) -> JsResult<Vec<String>> {
    if v.is_undefined() {
        return Ok(Vec::new());
    }
    let (it, next) = vm.get_iterator(v)?;
    let mut out = Vec::new();
    while let Some(x) = vm.iterator_step_value(&it, &next)? {
        match x {
            Value::String(s) => out.push(s.to_rust()),
            _ => {
                let _ = vm.iterator_close(&it);
                return vm.throw_type("Iterable yielded a value which is not a string");
            }
        }
    }
    Ok(out)
}

fn lf_format(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_lf, "Intl.ListFormat.prototype.format")?;
    let v = vm.arg(ctx, 0);
    let items = string_list(vm, &v)?;
    let f = lf_of(vm, o);
    Ok(Value::str(&concat_parts(&parts(&f, &items))))
}

fn lf_format_to_parts(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_lf, "Intl.ListFormat.prototype.formatToParts")?;
    let v = vm.arg(ctx, 0);
    let items = string_list(vm, &v)?;
    let f = lf_of(vm, o);
    let ps = parts(&f, &items);
    Ok(parts_array(vm, &ps, None))
}

fn lf_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_lf, "Intl.ListFormat.prototype.resolvedOptions")?;
    let f = lf_of(vm, o);
    let r = object_from(vm, vec![("locale", Value::str(&f.locale)), ("type", Value::str(f.kind)), ("style", Value::str(f.style))]);
    Ok(Value::Object(r))
}
