//! String (§22.1), the String iterator, and Annex B.2.2 additions.

use super::*;
use crate::string::{code_point_at, push_code_point};
use crate::unicode;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let mut d = ObjectData::new(Some(op), Kind::String(JsStr::empty()));
    d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(0.0), 0));
    let proto = vm.alloc(d);
    let c = ctor(vm, "String", 1, string_ctor, proto);
    method(vm, c, "fromCharCode", 1, from_char_code);
    method(vm, c, "fromCodePoint", 1, from_code_point);
    method(vm, c, "raw", 1, raw);
    for (n, l, f) in [
        ("at", 1, at as NativeFn),
        ("charAt", 1, char_at),
        ("charCodeAt", 1, char_code_at),
        ("codePointAt", 1, code_point_at_m),
        ("concat", 1, concat),
        ("endsWith", 1, ends_with),
        ("includes", 1, includes),
        ("indexOf", 1, index_of),
        ("isWellFormed", 0, is_well_formed),
        ("lastIndexOf", 1, last_index_of),
        ("localeCompare", 1, locale_compare),
        ("match", 1, match_),
        ("matchAll", 1, match_all),
        ("normalize", 0, normalize),
        ("padEnd", 1, pad_end),
        ("padStart", 1, pad_start),
        ("repeat", 1, repeat),
        ("replace", 2, replace),
        ("replaceAll", 2, replace_all),
        ("search", 1, search),
        ("slice", 2, slice),
        ("split", 2, split),
        ("startsWith", 1, starts_with),
        ("substr", 2, substr),
        ("substring", 2, substring),
        ("toLocaleLowerCase", 0, to_lower),
        ("toLocaleUpperCase", 0, to_upper),
        ("toLowerCase", 0, to_lower),
        ("toString", 0, to_string_m),
        ("toUpperCase", 0, to_upper),
        ("toWellFormed", 0, to_well_formed),
        ("trim", 0, trim),
        ("valueOf", 0, to_string_m),
    ] {
        method(vm, proto, n, l, f);
    }
    let ts = method(vm, proto, "trimStart", 0, trim_start);
    let te = method(vm, proto, "trimEnd", 0, trim_end);
    vm.heap.get_mut(proto).props.insert(PropertyKey::from_str("trimLeft"), Prop::data(Value::Object(ts), WC));
    vm.heap.get_mut(proto).props.insert(PropertyKey::from_str("trimRight"), Prop::data(Value::Object(te), WC));
    for (n, tag, attr) in [
        ("anchor", "a", Some("name")),
        ("big", "big", None),
        ("blink", "blink", None),
        ("bold", "b", None),
        ("fixed", "tt", None),
        ("fontcolor", "font", Some("color")),
        ("fontsize", "font", Some("size")),
        ("italics", "i", None),
        ("link", "a", Some("href")),
        ("small", "small", None),
        ("strike", "strike", None),
        ("sub", "sub", None),
        ("sup", "sup", None),
    ] {
        let f: NativeFn = html_method;
        let fo = vm.make_native_with(n, if attr.is_some() { 1 } else { 0 }, f, false, Some(vm.intr().function_proto), alloc::vec![Value::str(tag), attr.map(Value::str).unwrap_or(Value::Undefined)]);
        vm.heap.get_mut(proto).props.insert(PropertyKey::from_str(n), Prop::data(Value::Object(fo), WC));
    }
    let it = vm.wk.iterator.clone();
    method_sym(vm, proto, it, "[Symbol.iterator]", 0, string_iterator, WC);
    let ip = vm.intr().iterator_proto;
    let sip = vm.new_object(Some(ip));
    method(vm, sip, "next", 0, string_iter_next);
    to_str_tag(vm, sip, "String Iterator");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.string_iterator_proto = sip;
    vm.realms[r].intrinsics.string_proto = proto;
    vm.realms[r].intrinsics.string_ctor = c;
    global(vm, "String", Value::Object(c));
}

fn string_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = if ctx.argc == 0 {
        JsStr::empty()
    } else {
        let v = vm.arg(ctx, 0);
        if ctx.new_target.is_undefined() {
            if let Value::Symbol(sym) = &v {
                return Ok(Value::String(crate::builtins::symbol::descriptive_string(sym)));
            }
        }
        vm.to_string(&v)?
    };
    if ctx.new_target.is_undefined() {
        return Ok(Value::String(s));
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.string_proto)?;
    let mut d = ObjectData::new(Some(p), Kind::String(s.clone()));
    d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(s.len() as f64), 0));
    Ok(Value::Object(vm.alloc(d)))
}

/// RequireObjectCoercible(this) then ToString.
fn this_str(vm: &mut Vm, ctx: &CallCtx) -> JsResult<JsStr> {
    match &ctx.this {
        Value::String(s) => Ok(s.clone()),
        Value::Undefined | Value::Null => vm.throw_type("String.prototype method called on null or undefined"),
        v => {
            let v = v.clone();
            vm.to_string(&v)
        }
    }
}

fn from_char_code(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut v = Vec::with_capacity(ctx.argc);
    for i in 0..ctx.argc {
        let a = vm.arg(ctx, i);
        let n = vm.to_number(&a)?;
        v.push(crate::vm::ops::to_uint16(n));
    }
    Ok(Value::String(JsStr::from_units(v)))
}

fn from_code_point(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let mut v = Vec::with_capacity(ctx.argc);
    for i in 0..ctx.argc {
        let a = vm.arg(ctx, i);
        let n = vm.to_number(&a)?;
        if crate::numconv::libm_floor(n) != n || !(0.0..=1114111.0).contains(&n) {
            return vm.throw_range(&alloc::format!("Invalid code point {}", crate::numconv::f64_to_js_string(n)));
        }
        push_code_point(&mut v, n as u32);
    }
    Ok(Value::String(JsStr::from_units(v)))
}

fn raw(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = vm.arg(ctx, 0);
    let cooked = vm.to_object(&c)?.as_object().unwrap();
    let r = vm.get(cooked, &PropertyKey::from_str("raw"))?;
    let lit = vm.to_object(&r)?.as_object().unwrap();
    let len = vm.length_of(lit)?;
    if len <= 0.0 {
        return Ok(Value::String(JsStr::empty()));
    }
    let mut out: Vec<u16> = Vec::new();
    let mut i = 0.0;
    loop {
        let seg = vm.get(lit, &crate::builtins::array::key_of(i))?;
        let s = vm.to_string(&seg)?;
        out.extend_from_slice(s.units());
        if i + 1.0 == len {
            break;
        }
        if (i as usize + 1) < ctx.argc {
            let sub = vm.arg(ctx, i as usize + 1);
            let s = vm.to_string(&sub)?;
            out.extend_from_slice(s.units());
        }
        i += 1.0;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn at(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let a = vm.arg(ctx, 0);
    let rel = vm.to_integer_or_infinity(&a)?;
    let len = s.len() as f64;
    let k = if rel >= 0.0 { rel } else { len + rel };
    if k < 0.0 || k >= len {
        return Ok(Value::Undefined);
    }
    Ok(Value::String(s.slice(k as usize, k as usize + 1)))
}

fn char_at(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let a = vm.arg(ctx, 0);
    let p = vm.to_integer_or_infinity(&a)?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::String(JsStr::empty()));
    }
    Ok(Value::String(s.slice(p as usize, p as usize + 1)))
}

fn char_code_at(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let a = vm.arg(ctx, 0);
    let p = vm.to_integer_or_infinity(&a)?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::Number(f64::NAN));
    }
    Ok(Value::Number(s.units()[p as usize] as f64))
}

fn code_point_at_m(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let a = vm.arg(ctx, 0);
    let p = vm.to_integer_or_infinity(&a)?;
    if p < 0.0 || p >= s.len() as f64 {
        return Ok(Value::Undefined);
    }
    Ok(Value::Number(code_point_at(s.units(), p as usize).0 as f64))
}

fn concat(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let mut out = s.units().to_vec();
    for i in 0..ctx.argc {
        let a = vm.arg(ctx, i);
        let t = vm.to_string(&a)?;
        out.extend_from_slice(t.units());
        if out.len() > 1 << 30 {
            return vm.throw_range("Invalid string length");
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

/// IsRegExp (§7.2.8)
pub fn is_regexp(vm: &mut Vm, v: &Value) -> JsResult<bool> {
    let o = match v {
        Value::Object(o) => *o,
        _ => return Ok(false),
    };
    let k = PropertyKey::Sym(vm.wk.match_.clone());
    let m = vm.get(o, &k)?;
    if !m.is_undefined() {
        return Ok(vm.to_boolean(&m));
    }
    Ok(matches!(vm.heap.get(o).kind, Kind::RegExp(_)))
}

pub fn index_of_units(h: &[u16], n: &[u16], from: usize) -> Option<usize> {
    if n.is_empty() {
        return if from <= h.len() { Some(from) } else { None };
    }
    if n.len() > h.len() {
        return None;
    }
    (from..=h.len() - n.len()).find(|&i| &h[i..i + n.len()] == n)
}

fn ends_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let sv = vm.arg(ctx, 0);
    if is_regexp(vm, &sv)? {
        return vm.throw_type("First argument to String.prototype.endsWith must not be a regular expression");
    }
    let search = vm.to_string(&sv)?;
    let len = s.len() as f64;
    let ep = vm.arg(ctx, 1);
    let end = if ep.is_undefined() { len } else { vm.to_integer_or_infinity(&ep)?.clamp(0.0, len) };
    let sl = search.len() as f64;
    let start = end - sl;
    if start < 0.0 {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(&s.units()[start as usize..end as usize] == search.units()))
}

fn starts_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let sv = vm.arg(ctx, 0);
    if is_regexp(vm, &sv)? {
        return vm.throw_type("First argument to String.prototype.startsWith must not be a regular expression");
    }
    let search = vm.to_string(&sv)?;
    let len = s.len() as f64;
    let pp = vm.arg(ctx, 1);
    let start = vm.to_integer_or_infinity(&pp)?.clamp(0.0, len);
    let end = start + search.len() as f64;
    if end > len {
        return Ok(Value::Bool(false));
    }
    Ok(Value::Bool(&s.units()[start as usize..end as usize] == search.units()))
}

fn includes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let sv = vm.arg(ctx, 0);
    if is_regexp(vm, &sv)? {
        return vm.throw_type("First argument to String.prototype.includes must not be a regular expression");
    }
    let search = vm.to_string(&sv)?;
    let pp = vm.arg(ctx, 1);
    let start = vm.to_integer_or_infinity(&pp)?.clamp(0.0, s.len() as f64);
    Ok(Value::Bool(index_of_units(s.units(), search.units(), start as usize).is_some()))
}

fn index_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let sv = vm.arg(ctx, 0);
    let search = vm.to_string(&sv)?;
    let pp = vm.arg(ctx, 1);
    let start = vm.to_integer_or_infinity(&pp)?.clamp(0.0, s.len() as f64);
    Ok(Value::Number(index_of_units(s.units(), search.units(), start as usize).map(|i| i as f64).unwrap_or(-1.0)))
}

fn last_index_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let sv = vm.arg(ctx, 0);
    let search = vm.to_string(&sv)?;
    let pp = vm.arg(ctx, 1);
    let num = vm.to_number(&pp)?;
    let pos = if num.is_nan() { f64::INFINITY } else { crate::vm::ops::integer_or_infinity(num) };
    let len = s.len();
    let sl = search.len();
    if sl > len {
        return Ok(Value::Number(-1.0));
    }
    let start = pos.clamp(0.0, (len - sl) as f64) as usize;
    let h = s.units();
    let n = search.units();
    let mut i = start as isize;
    while i >= 0 {
        let iu = i as usize;
        if &h[iu..iu + sl] == n {
            return Ok(Value::Number(iu as f64));
        }
        i -= 1;
    }
    Ok(Value::Number(-1.0))
}

fn is_well_formed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    Ok(Value::Bool(char::decode_utf16(s.units().iter().copied()).all(|r| r.is_ok())))
}

fn to_well_formed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let mut out = Vec::with_capacity(s.len());
    for r in char::decode_utf16(s.units().iter().copied()) {
        push_code_point(&mut out, r.map(|c| c as u32).unwrap_or(0xFFFD));
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn code_points(s: &[u16]) -> Vec<u32> {
    let mut out = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < s.len() {
        let (c, n) = code_point_at(s, i);
        out.push(c);
        i += n;
    }
    out
}

fn from_code_points(cps: &[u32]) -> JsStr {
    let mut v = Vec::with_capacity(cps.len());
    for &c in cps {
        push_code_point(&mut v, c);
    }
    JsStr::from_units(v)
}

fn locale_compare(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let ta = vm.arg(ctx, 0);
    let t = vm.to_string(&ta)?;
    // Without Intl: compare canonically equivalent strings as equal (NFD), ordering by code point.
    let a = unicode::normalize(&code_points(s.units()), 1);
    let b = unicode::normalize(&code_points(t.units()), 1);
    Ok(Value::Number(match a.cmp(&b) {
        core::cmp::Ordering::Less => -1.0,
        core::cmp::Ordering::Equal => 0.0,
        core::cmp::Ordering::Greater => 1.0,
    }))
}

fn normalize(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let fa = vm.arg(ctx, 0);
    let form = if fa.is_undefined() { JsStr::from_str("NFC") } else { vm.to_string(&fa)? };
    let f = match form.to_rust().as_str() {
        "NFC" => 0,
        "NFD" => 1,
        "NFKC" => 2,
        "NFKD" => 3,
        _ => return vm.throw_range("The normalization form should be one of NFC, NFD, NFKC, NFKD."),
    };
    let r = unicode::normalize(&code_points(s.units()), f);
    Ok(Value::String(from_code_points(&r)))
}

fn pad(vm: &mut Vm, ctx: &CallCtx, at_start: bool) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let ml = vm.arg(ctx, 0);
    let max_len = vm.to_length(&ml)?;
    let len = s.len() as f64;
    if max_len <= len {
        return Ok(Value::String(s));
    }
    let fs = vm.arg(ctx, 1);
    let filler = if fs.is_undefined() { JsStr::from_str(" ") } else { vm.to_string(&fs)? };
    if filler.is_empty() {
        return Ok(Value::String(s));
    }
    if max_len > (1u64 << 30) as f64 {
        return vm.throw_range("Invalid string length");
    }
    let fill_len = (max_len - len) as usize;
    let mut f: Vec<u16> = Vec::with_capacity(fill_len);
    while f.len() < fill_len {
        let take = (fill_len - f.len()).min(filler.len());
        f.extend_from_slice(&filler.units()[..take]);
    }
    let mut out = Vec::with_capacity(max_len as usize);
    if at_start {
        out.extend_from_slice(&f);
        out.extend_from_slice(s.units());
    } else {
        out.extend_from_slice(s.units());
        out.extend_from_slice(&f);
    }
    Ok(Value::String(JsStr::from_units(out)))
}
fn pad_end(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    pad(vm, ctx, false)
}
fn pad_start(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    pad(vm, ctx, true)
}

fn repeat(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let ca = vm.arg(ctx, 0);
    let n = vm.to_integer_or_infinity(&ca)?;
    if n < 0.0 || n == f64::INFINITY {
        return vm.throw_range("Invalid count value");
    }
    if n == 0.0 || s.is_empty() {
        return Ok(Value::String(JsStr::empty()));
    }
    if s.len() as f64 * n > (1u64 << 30) as f64 {
        return vm.throw_range("Invalid string length");
    }
    let mut out = Vec::with_capacity(s.len() * n as usize);
    for _ in 0..n as usize {
        out.extend_from_slice(s.units());
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn slice(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let len = s.len() as f64;
    let a = vm.arg(ctx, 0);
    let from = relative_index(vm, &a, len, 0.0)?;
    let b = vm.arg(ctx, 1);
    let to = relative_index(vm, &b, len, len)?;
    if from >= to {
        return Ok(Value::String(JsStr::empty()));
    }
    Ok(Value::String(s.slice(from as usize, to as usize)))
}

fn substring(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let len = s.len() as f64;
    let a = vm.arg(ctx, 0);
    let st = vm.to_integer_or_infinity(&a)?.clamp(0.0, len);
    let b = vm.arg(ctx, 1);
    let en = if b.is_undefined() { len } else { vm.to_integer_or_infinity(&b)?.clamp(0.0, len) };
    let (f, t) = if st < en { (st, en) } else { (en, st) };
    Ok(Value::String(s.slice(f as usize, t as usize)))
}

fn substr(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let size = s.len() as f64;
    let a = vm.arg(ctx, 0);
    let mut start = vm.to_integer_or_infinity(&a)?;
    if start == f64::NEG_INFINITY {
        start = 0.0;
    } else if start < 0.0 {
        start = (size + start).max(0.0);
    } else {
        start = start.min(size);
    }
    let b = vm.arg(ctx, 1);
    let length = if b.is_undefined() { size } else { vm.to_integer_or_infinity(&b)? };
    let end = (start + length).min(size);
    if start >= end {
        return Ok(Value::String(JsStr::empty()));
    }
    Ok(Value::String(s.slice(start as usize, end as usize)))
}

fn to_string_m(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    match &ctx.this {
        Value::String(s) => Ok(Value::String(s.clone())),
        Value::Object(o) => match &vm.heap.get(*o).kind {
            Kind::String(s) => Ok(Value::String(s.clone())),
            _ => vm.throw_type("String.prototype.toString requires that 'this' be a String"),
        },
        _ => vm.throw_type("String.prototype.toString requires that 'this' be a String"),
    }
}

pub fn to_lower_str(s: &[u16]) -> Vec<u16> {
    let cps = code_points(s);
    let mut out = Vec::with_capacity(s.len());
    let mut buf = Vec::new();
    for (i, &c) in cps.iter().enumerate() {
        if c == 0x3A3 {
            // Final_Sigma: preceded by a cased letter (skipping case-ignorable) and not followed by one.
            let before = cps[..i].iter().rev().find(|&&x| !unicode::is_case_ignorable(x)).map(|&x| unicode::is_cased(x)).unwrap_or(false);
            let after = cps[i + 1..].iter().find(|&&x| !unicode::is_case_ignorable(x)).map(|&x| unicode::is_cased(x)).unwrap_or(false);
            push_code_point(&mut out, if before && !after { 0x3C2 } else { 0x3C3 });
            continue;
        }
        buf.clear();
        unicode::to_lower_full(c, &mut buf);
        for &x in &buf {
            push_code_point(&mut out, x);
        }
    }
    out
}

pub fn to_upper_str(s: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(s.len());
    let mut buf = Vec::new();
    let mut i = 0;
    while i < s.len() {
        let (c, n) = code_point_at(s, i);
        buf.clear();
        unicode::to_upper_full(c, &mut buf);
        for &x in &buf {
            push_code_point(&mut out, x);
        }
        i += n;
    }
    out
}

fn to_lower(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    Ok(Value::String(JsStr::from_units(to_lower_str(s.units()))))
}
fn to_upper(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    Ok(Value::String(JsStr::from_units(to_upper_str(s.units()))))
}

fn trim_units(s: &[u16], start: bool, end: bool) -> (usize, usize) {
    let mut a = 0;
    let mut b = s.len();
    if start {
        while a < b && unicode::is_str_whitespace(s[a] as u32) {
            a += 1;
        }
    }
    if end {
        while b > a && unicode::is_str_whitespace(s[b - 1] as u32) {
            b -= 1;
        }
    }
    (a, b)
}
fn trim(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let (a, b) = trim_units(s.units(), true, true);
    Ok(Value::String(s.slice(a, b)))
}
fn trim_start(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let (a, b) = trim_units(s.units(), true, false);
    Ok(Value::String(s.slice(a, b)))
}
fn trim_end(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let (a, b) = trim_units(s.units(), false, true);
    Ok(Value::String(s.slice(a, b)))
}

fn html_method(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let tag = vm.native_slot(ctx.callee, 0);
    let attr = vm.native_slot(ctx.callee, 1);
    let tag = match tag {
        Value::String(t) => t.to_rust(),
        _ => alloc::string::String::new(),
    };
    let mut p = alloc::format!("<{}", tag);
    if let Value::String(a) = attr {
        let v = vm.arg(ctx, 0);
        let vs = vm.to_string(&v)?.to_rust().replace('"', "&quot;");
        p.push_str(&alloc::format!(" {}=\"{}\"", a, vs));
    }
    p.push('>');
    let mut out: Vec<u16> = p.encode_utf16().collect();
    out.extend_from_slice(s.units());
    out.extend(alloc::format!("</{}>", tag).encode_utf16());
    Ok(Value::String(JsStr::from_units(out)))
}

// ------------------------------------------------------------------------------------------------ regexp-dispatching methods

/// Call `regexp[@@sym](this, ...rest)` when the argument provides the method (§22.1.3.12 etc.).
fn dispatch(vm: &mut Vm, ctx: &CallCtx, sym: Sym, args: &[Value]) -> JsResult<Option<Value>> {
    let rx = vm.arg(ctx, 0);
    if rx.is_object() {
        let m = vm.get_method(&rx, &PropertyKey::Sym(sym))?;
        if let Some(m) = m {
            return Ok(Some(vm.call(&m, &rx, args)?));
        }
    }
    Ok(None)
}

fn require_coercible(vm: &mut Vm, ctx: &CallCtx) -> JsResult<()> {
    if ctx.this.is_nullish() {
        return vm.throw_type("String.prototype method called on null or undefined");
    }
    Ok(())
}

fn match_(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_coercible(vm, ctx)?;
    let sym = vm.wk.match_.clone();
    let this = ctx.this.clone();
    if let Some(r) = dispatch(vm, ctx, sym.clone(), &[this])? {
        return Ok(r);
    }
    let s = this_str(vm, ctx)?;
    let rx = vm.arg(ctx, 0);
    let re = crate::builtins::regexp::regexp_create(vm, rx, Value::Undefined)?;
    vm.invoke(&Value::Object(re), &PropertyKey::Sym(sym), &[Value::String(s)])
}

fn match_all(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_coercible(vm, ctx)?;
    let rx = vm.arg(ctx, 0);
    if !rx.is_nullish() && is_regexp(vm, &rx)? {
        let flags = vm.get_v(&rx, &PropertyKey::from_str("flags"))?;
        if flags.is_nullish() {
            return vm.throw_type("RegExp flags is null or undefined");
        }
        let f = vm.to_string(&flags)?;
        if !f.units().contains(&(b'g' as u16)) {
            return vm.throw_type("String.prototype.matchAll called with a non-global RegExp argument");
        }
    }
    let sym = vm.wk.match_all.clone();
    let this = ctx.this.clone();
    if let Some(r) = dispatch(vm, ctx, sym.clone(), &[this])? {
        return Ok(r);
    }
    let s = this_str(vm, ctx)?;
    let re = crate::builtins::regexp::regexp_create(vm, rx, Value::str("g"))?;
    vm.invoke(&Value::Object(re), &PropertyKey::Sym(sym), &[Value::String(s)])
}

fn search(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_coercible(vm, ctx)?;
    let sym = vm.wk.search.clone();
    let this = ctx.this.clone();
    if let Some(r) = dispatch(vm, ctx, sym.clone(), &[this])? {
        return Ok(r);
    }
    let s = this_str(vm, ctx)?;
    let rx = vm.arg(ctx, 0);
    let re = crate::builtins::regexp::regexp_create(vm, rx, Value::Undefined)?;
    vm.invoke(&Value::Object(re), &PropertyKey::Sym(sym), &[Value::String(s)])
}

/// GetSubstitution (§22.1.3.19.1)
pub fn get_substitution(vm: &mut Vm, matched: &JsStr, s: &JsStr, position: usize, captures: &[Value], named: &Value, replacement: &JsStr) -> JsResult<JsStr> {
    let r = replacement.units();
    let su = s.units();
    let m = captures.len();
    let tail_pos = (position + matched.len()).min(su.len());
    let mut out: Vec<u16> = Vec::with_capacity(r.len());
    let mut i = 0;
    while i < r.len() {
        let c = r[i];
        if c == b'$' as u16 && i + 1 < r.len() {
            let n = r[i + 1];
            match n {
                0x24 => {
                    out.push(0x24);
                    i += 2;
                    continue;
                }
                0x26 => {
                    out.extend_from_slice(matched.units());
                    i += 2;
                    continue;
                }
                0x60 => {
                    out.extend_from_slice(&su[..position.min(su.len())]);
                    i += 2;
                    continue;
                }
                0x27 => {
                    out.extend_from_slice(&su[tail_pos..]);
                    i += 2;
                    continue;
                }
                0x30..=0x39 => {
                    let d1 = (n - 0x30) as usize;
                    let two = if i + 2 < r.len() && (0x30..=0x39).contains(&r[i + 2]) { Some(d1 * 10 + (r[i + 2] - 0x30) as usize) } else { None };
                    if let Some(t) = two {
                        if t >= 1 && t <= m {
                            let v = &captures[t - 1];
                            if !v.is_undefined() {
                                let cs = vm.to_string(v)?;
                                out.extend_from_slice(cs.units());
                            }
                            i += 3;
                            continue;
                        }
                    }
                    if d1 >= 1 && d1 <= m {
                        let v = &captures[d1 - 1];
                        if !v.is_undefined() {
                            let cs = vm.to_string(v)?;
                            out.extend_from_slice(cs.units());
                        }
                        i += 2;
                        continue;
                    }
                }
                0x3C => {
                    if named.is_undefined() {
                        out.push(c);
                        i += 1;
                        continue;
                    }
                    let close = r[i + 2..].iter().position(|&x| x == b'>' as u16);
                    match close {
                        None => {
                            out.push(c);
                            i += 1;
                            continue;
                        }
                        Some(cl) => {
                            let group = JsStr::from_slice(&r[i + 2..i + 2 + cl]);
                            let gv = vm.get_v(named, &PropertyKey::from_js(group))?;
                            if !gv.is_undefined() {
                                let gs = vm.to_string(&gv)?;
                                out.extend_from_slice(gs.units());
                            }
                            i += 3 + cl;
                            continue;
                        }
                    }
                }
                _ => {}
            }
        }
        out.push(c);
        i += 1;
    }
    Ok(JsStr::from_units(out))
}

fn replace_impl(vm: &mut Vm, ctx: &CallCtx, all: bool) -> JsResult<Value> {
    require_coercible(vm, ctx)?;
    let search_value = vm.arg(ctx, 0);
    let replace_value = vm.arg(ctx, 1);
    if all && !search_value.is_nullish() && is_regexp(vm, &search_value)? {
        let flags = vm.get_v(&search_value, &PropertyKey::from_str("flags"))?;
        if flags.is_nullish() {
            return vm.throw_type("RegExp flags is null or undefined");
        }
        let f = vm.to_string(&flags)?;
        if !f.units().contains(&(b'g' as u16)) {
            return vm.throw_type("String.prototype.replaceAll called with a non-global RegExp argument");
        }
    }
    let sym = vm.wk.replace.clone();
    let this = ctx.this.clone();
    if let Some(r) = dispatch(vm, ctx, sym, &[this, replace_value.clone()])? {
        return Ok(r);
    }
    let s = this_str(vm, ctx)?;
    let search = vm.to_string(&search_value)?;
    let functional = vm.is_callable(&replace_value);
    let replace_str = if functional { JsStr::empty() } else { vm.to_string(&replace_value)? };
    let sl = search.len();
    let adv = sl.max(1);
    let mut positions = Vec::new();
    let mut p = index_of_units(s.units(), search.units(), 0);
    while let Some(pos) = p {
        positions.push(pos);
        if !all {
            break;
        }
        p = index_of_units(s.units(), search.units(), pos + adv);
    }
    let mut end = 0;
    let mut out: Vec<u16> = Vec::new();
    for pos in positions {
        out.extend_from_slice(&s.units()[end..pos]);
        let rep = if functional {
            let r = vm.call(&replace_value, &Value::Undefined, &[Value::String(search.clone()), Value::Number(pos as f64), Value::String(s.clone())])?;
            vm.to_string(&r)?
        } else {
            get_substitution(vm, &search, &s, pos, &[], &Value::Undefined, &replace_str)?
        };
        out.extend_from_slice(rep.units());
        end = pos + sl;
    }
    if end == 0 && out.is_empty() && index_of_units(s.units(), search.units(), 0).is_none() {
        return Ok(Value::String(s));
    }
    out.extend_from_slice(&s.units()[end.min(s.len())..]);
    Ok(Value::String(JsStr::from_units(out)))
}
fn replace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    replace_impl(vm, ctx, false)
}
fn replace_all(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    replace_impl(vm, ctx, true)
}

fn split(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    require_coercible(vm, ctx)?;
    let sym = vm.wk.split.clone();
    let this = ctx.this.clone();
    let limit = vm.arg(ctx, 1);
    if let Some(r) = dispatch(vm, ctx, sym, &[this, limit.clone()])? {
        return Ok(r);
    }
    let s = this_str(vm, ctx)?;
    let lim = if limit.is_undefined() { u32::MAX } else { vm.to_uint32(&limit)? };
    let sep_v = vm.arg(ctx, 0);
    let sep = vm.to_string(&sep_v)?;
    if lim == 0 {
        return Ok(Value::Object(vm.new_array(Vec::new())));
    }
    if sep_v.is_undefined() {
        return Ok(Value::Object(vm.new_array(alloc::vec![Value::String(s)])));
    }
    let su = s.units();
    if su.is_empty() {
        if !sep.is_empty() {
            return Ok(Value::Object(vm.new_array(alloc::vec![Value::String(s)])));
        }
        return Ok(Value::Object(vm.new_array(Vec::new())));
    }
    let mut parts = Vec::new();
    if sep.is_empty() {
        for i in 0..su.len() {
            if parts.len() as u32 >= lim {
                break;
            }
            parts.push(Value::String(s.slice(i, i + 1)));
        }
        return Ok(Value::Object(vm.new_array(parts)));
    }
    let mut p = 0;
    let mut q = index_of_units(su, sep.units(), 0);
    while let Some(pos) = q {
        parts.push(Value::String(s.slice(p, pos)));
        if parts.len() as u32 >= lim {
            return Ok(Value::Object(vm.new_array(parts)));
        }
        p = pos + sep.len();
        q = index_of_units(su, sep.units(), p);
    }
    parts.push(Value::String(s.slice(p, su.len())));
    Ok(Value::Object(vm.new_array(parts)))
}

// ------------------------------------------------------------------------------------------------ iterator

fn string_iterator(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = this_str(vm, ctx)?;
    let p = vm.intr().string_iterator_proto;
    Ok(Value::Object(vm.alloc(ObjectData::new(Some(p), Kind::Iterator(Box::new(IterData::String { s, pos: 0, done: false }))))))
}

fn string_iter_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("next called on incompatible receiver"),
    };
    let r = match &mut vm.heap.get_mut(o).kind {
        Kind::Iterator(d) => match &mut **d {
            IterData::String { s, pos, done } => {
                if *done || *pos >= s.len() {
                    *done = true;
                    None
                } else {
                    let (_, n) = code_point_at(s.units(), *pos);
                    let v = s.slice(*pos, *pos + n);
                    *pos += n;
                    Some(v)
                }
            }
            _ => return vm.throw_type("next called on incompatible receiver"),
        },
        _ => return vm.throw_type("next called on incompatible receiver"),
    };
    Ok(Value::Object(match r {
        Some(v) => vm.iter_result(Value::String(v), false),
        None => vm.iter_result(Value::Undefined, true),
    }))
}
