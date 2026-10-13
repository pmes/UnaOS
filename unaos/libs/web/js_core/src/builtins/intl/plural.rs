//! Intl.PluralRules (ECMA-402 §16) with the CLDR `en` rules: cardinal `one` (i = 1 and v = 0) / `other`;
//! ordinal `one` (n % 10 = 1, n % 100 ≠ 11), `two` (2, ≠ 12), `few` (3, ≠ 13), `other`.

use super::number::{digit_options, format_numeric, mv_of, Digits, Mv, RType};
use super::*;

#[derive(Clone, Debug)]
pub struct Plural {
    pub locale: String,
    pub ordinal: bool,
    pub dg: Digits,
}

pub fn init(vm: &mut Vm, intl: Obj) {
    let (_c, p) = service(vm, intl, "PluralRules", 0, pr_ctor);
    method(vm, p, "select", 1, pr_select);
    method(vm, p, "selectRange", 2, pr_select_range);
    method(vm, p, "resolvedOptions", 0, pr_resolved_options);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.intl_plural_rules_proto = p;
}

fn pr_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_new(vm, ctx, "PluralRules")?;
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.intl_plural_rules_proto)?;
    let l = vm.arg(ctx, 0);
    let requested = canonicalize_locale_list(vm, &l)?;
    let ov = vm.arg(ctx, 1);
    let o = coerce_options(vm, &ov)?;
    get_string_option(vm, o, "localeMatcher", &["lookup", "best fit"], "Intl.PluralRules")?;
    let t = get_string_option(vm, o, "type", &["cardinal", "ordinal"], "Intl.PluralRules")?;
    let dg = digit_options(vm, o, 0, 3, false)?;
    let r = resolve_locale(&requested, &[]);
    let pr = Plural { locale: String::from(r.data_locale), ordinal: t.as_deref() == Some("ordinal"), dg };
    Ok(Value::Object(alloc_intl(vm, p, IntlData::PluralRules(Box::new(pr)))))
}

/// ResolvePlural (§16.5.3) for `en`.
pub fn select(pr: &Plural, mv: &Mv) -> &'static str {
    let x = match mv {
        Mv::Fin(x) => x,
        _ => return "other",
    };
    let (_, int, frac) = format_numeric(&pr.dg, x);
    let int = int.trim_start_matches('0');
    if !pr.ordinal {
        return if int == "1" && frac.is_empty() { "one" } else { "other" };
    }
    if frac.bytes().any(|b| b != b'0') {
        return "other";
    }
    let b = int.as_bytes();
    let last = b.last().map(|c| c - b'0').unwrap_or(0);
    let tens = if b.len() >= 2 { b[b.len() - 2] - b'0' } else { 0 };
    match (last, tens) {
        (1, t) if t != 1 => "one",
        (2, t) if t != 1 => "two",
        (3, t) if t != 1 => "few",
        _ => "other",
    }
}

fn is_pr(d: &IntlData) -> bool {
    matches!(d, IntlData::PluralRules(_))
}

fn pr_of(vm: &Vm, o: Obj) -> Plural {
    match intl_ref(vm, o) {
        IntlData::PluralRules(p) => (**p).clone(),
        _ => unreachable!(),
    }
}

fn pr_select(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_pr, "Intl.PluralRules.prototype.select")?;
    let v = vm.arg(ctx, 0);
    let n = vm.to_number(&v)?;
    let pr = pr_of(vm, o);
    Ok(Value::str(select(&pr, &mv_of(n))))
}

fn pr_select_range(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_pr, "Intl.PluralRules.prototype.selectRange")?;
    let a = vm.arg(ctx, 0);
    let b = vm.arg(ctx, 1);
    if a.is_undefined() || b.is_undefined() {
        return vm.throw_type("start or end is undefined");
    }
    let x = vm.to_number(&a)?;
    let y = vm.to_number(&b)?;
    if x.is_nan() || y.is_nan() {
        return vm.throw_range("Invalid number range: NaN");
    }
    let _ = pr_of(vm, o);
    // CLDR `en` plural ranges: one+other, other+one and other+other all resolve to `other`.
    Ok(Value::str("other"))
}

fn pr_resolved_options(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_intl(vm, &ctx.this, is_pr, "Intl.PluralRules.prototype.resolvedOptions")?;
    let pr = pr_of(vm, o);
    let d = &pr.dg;
    let mut props: Vec<(&str, Value)> = vec![
        ("locale", Value::str(&pr.locale)),
        ("type", Value::str(if pr.ordinal { "ordinal" } else { "cardinal" })),
        ("minimumIntegerDigits", Value::Number(d.min_int as f64)),
    ];
    if d.rtype != RType::Sig {
        props.push(("minimumFractionDigits", Value::Number(d.min_frac as f64)));
        props.push(("maximumFractionDigits", Value::Number(d.max_frac as f64)));
    }
    if d.rtype != RType::Frac {
        props.push(("minimumSignificantDigits", Value::Number(d.min_sig as f64)));
        props.push(("maximumSignificantDigits", Value::Number(d.max_sig as f64)));
    }
    let cats: &[&str] = if pr.ordinal { &["one", "two", "few", "other"] } else { &["one", "other"] };
    let vals = cats.iter().map(|c| Value::str(c)).collect();
    let arr = vm.new_array(vals);
    props.push(("pluralCategories", Value::Object(arr)));
    props.push(("roundingIncrement", Value::Number(d.increment as f64)));
    props.push(("roundingMode", Value::str(d.mode.name())));
    props.push(("roundingPriority", Value::str(d.priority)));
    props.push(("trailingZeroDisplay", Value::str(if d.strip_if_integer { "stripIfInteger" } else { "auto" })));
    let r = object_from(vm, props);
    Ok(Value::Object(r))
}
