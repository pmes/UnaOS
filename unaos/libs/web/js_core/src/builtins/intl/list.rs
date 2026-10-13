//! Intl.ListFormat (ECMA-402 §13) with the CLDR `en` / `en-GB` list patterns.

use super::*;

#[derive(Clone, Debug)]
pub struct ListFmt {
    pub locale: String,
    pub gb: bool,
    pub kind: &'static str,
    pub style: &'static str,
}

/// (pair, start/middle, end) patterns: the separators between items.
fn patterns(f: &ListFmt) -> (&'static str, &'static str, &'static str) {
    match (f.kind, f.style, f.gb) {
        ("conjunction", "long", false) => (" and ", ", ", ", and "),
        ("conjunction", "long", true) => (" and ", ", ", " and "),
        ("conjunction", "short", false) => (" & ", ", ", ", & "),
        ("conjunction", "short", true) => (" and ", ", ", " and "),
        ("conjunction", _, _) => (", ", ", ", ", "),
        ("disjunction", _, false) => (" or ", ", ", ", or "),
        ("disjunction", _, true) => (" or ", ", ", " or "),
        ("unit", "narrow", _) => (" ", " ", " "),
        _ => (", ", ", ", ", "),
    }
}

/// CreatePartsFromList (§13.5.2).
pub fn parts(f: &ListFmt, items: &[String]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let n = items.len();
    let (pair, mid, end) = patterns(f);
    for (i, it) in items.iter().enumerate() {
        if i > 0 {
            let sep = if n == 2 { pair } else if i == n - 1 { end } else { mid };
            out.push((String::from("literal"), String::from(sep)));
        }
        out.push((String::from("element"), it.clone()));
    }
    out
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
        gb: is_gb(r.data_locale),
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
