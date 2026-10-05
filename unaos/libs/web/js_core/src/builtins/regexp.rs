//! RegExp (§22.2.4–§22.2.9): constructor, RegExpBuiltinExec, the prototype's flag accessors and the
//! @@match/@@matchAll/@@replace/@@search/@@split protocol, %RegExpStringIteratorPrototype%, RegExp.escape and
//! Annex B `compile`.

use super::*;
use crate::regexp::matcher::{self, MatchResult};
use crate::regexp::parser::Flags;
use crate::regexp::{Compiled, RegExpData};

/// Backtracking step budget per exec (a pathological pattern throws a RangeError instead of hanging).
pub const STEP_LIMIT: u64 = 200_000_000;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, "RegExp", 2, regexp_ctor, proto);
    species_getter(vm, c);
    method(vm, c, "escape", 1, regexp_escape);
    method(vm, proto, "exec", 1, exec);
    method(vm, proto, "test", 1, test);
    method(vm, proto, "toString", 0, to_string);
    method(vm, proto, "compile", 2, compile);
    let getters: [(&str, NativeFn); 10] = [
        ("dotAll", get_dot_all),
        ("flags", get_flags),
        ("global", get_global),
        ("hasIndices", get_has_indices),
        ("ignoreCase", get_ignore_case),
        ("multiline", get_multiline),
        ("source", get_source),
        ("sticky", get_sticky),
        ("unicode", get_unicode),
        ("unicodeSets", get_unicode_sets),
    ];
    for (n, f) in getters {
        accessor(vm, proto, PropertyKey::from_str(n), n, Some(f), None, C);
    }
    let wk = vm.wk.clone_syms();
    method_sym(vm, proto, wk.0, "[Symbol.match]", 1, sym_match, WC);
    method_sym(vm, proto, wk.1, "[Symbol.matchAll]", 1, sym_match_all, WC);
    method_sym(vm, proto, wk.2, "[Symbol.replace]", 2, sym_replace, WC);
    method_sym(vm, proto, wk.3, "[Symbol.search]", 1, sym_search, WC);
    method_sym(vm, proto, wk.4, "[Symbol.split]", 2, sym_split, WC);
    let ip = vm.intr().iterator_proto;
    let rsip = vm.new_object(Some(ip));
    method(vm, rsip, "next", 0, rsi_next);
    to_str_tag(vm, rsip, "RegExp String Iterator");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.regexp_proto = proto;
    vm.realms[r].intrinsics.regexp_ctor = c;
    vm.realms[r].intrinsics.regexp_string_iterator_proto = rsip;
    global(vm, "RegExp", Value::Object(c));
}

impl WellKnown {
    fn clone_syms(&self) -> (Sym, Sym, Sym, Sym, Sym) {
        (self.match_.clone(), self.match_all.clone(), self.replace.clone(), self.search.clone(), self.split.clone())
    }
}

fn data(vm: &Vm, v: &Value) -> Option<(JsStr, JsStr, Flags, Rc<Compiled>)> {
    if let Value::Object(o) = v {
        if let Kind::RegExp(d) = &vm.heap.get(*o).kind {
            return Some((d.source.clone(), d.flags.clone(), d.parsed, d.re.clone()));
        }
    }
    None
}

fn regexp_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let pattern = vm.arg(ctx, 0);
    let flags = vm.arg(ctx, 1);
    let pattern_is_regexp = super::string::is_regexp(vm, &pattern)?;
    let new_target = if ctx.new_target.is_undefined() {
        let nt = Value::Object(ctx.callee);
        if pattern_is_regexp && flags.is_undefined() {
            let pc = vm.get_v(&pattern, &PropertyKey::from_str("constructor"))?;
            if pc.same_value(&nt) {
                return Ok(pattern);
            }
        }
        nt
    } else {
        ctx.new_target.clone()
    };
    let (p, f) = if let Some((src, fl, _, _)) = data(vm, &pattern) {
        (Value::String(src), if flags.is_undefined() { Value::String(fl) } else { flags })
    } else if pattern_is_regexp {
        let src = vm.get_v(&pattern, &PropertyKey::from_str("source"))?;
        let f = if flags.is_undefined() { vm.get_v(&pattern, &PropertyKey::from_str("flags"))? } else { flags };
        (src, f)
    } else {
        (pattern, flags)
    };
    let proto = vm.get_prototype_from_ctor(&new_target, |i| i.regexp_proto)?;
    Ok(Value::Object(alloc_init(vm, proto, p, f)?))
}

fn parse_source(vm: &mut Vm, p: &Value, f: &Value) -> JsResult<RegExpData> {
    let src = if p.is_undefined() { JsStr::empty() } else { vm.to_string(p)? };
    let flags = if f.is_undefined() { JsStr::empty() } else { vm.to_string(f)? };
    make_data(vm, src, flags)
}

fn make_data(vm: &mut Vm, src: JsStr, flags: JsStr) -> JsResult<RegExpData> {
    let fl = match Flags::parse(flags.units()) {
        Ok(f) => f,
        Err(m) => return vm.throw_syntax(&alloc::format!("Invalid regular expression flags '{}': {}", flags, m)),
    };
    let re = match Compiled::new(src.units(), fl) {
        Ok(r) => r,
        Err(m) => return vm.throw_syntax(&alloc::format!("Invalid regular expression: /{}/: {}", src, m)),
    };
    Ok(RegExpData { source: src, flags, parsed: fl, re: Rc::new(re) })
}

fn alloc_init(vm: &mut Vm, proto: Obj, p: Value, f: Value) -> JsResult<Obj> {
    let d = parse_source(vm, &p, &f)?;
    let mut od = ObjectData::new(Some(proto), Kind::RegExp(Box::new(d)));
    od.props.insert(PropertyKey::from_str("lastIndex"), Prop::data(Value::Number(0.0), W));
    Ok(vm.alloc(od))
}

/// RegExpCreate(P, F)
pub fn regexp_create(vm: &mut Vm, p: Value, f: Value) -> JsResult<Obj> {
    let proto = vm.intr().regexp_proto;
    alloc_init(vm, proto, p, f)
}

pub fn regexp_create_literal(vm: &mut Vm, p: JsStr, f: JsStr) -> JsResult<Obj> {
    let proto = vm.intr().regexp_proto;
    let d = make_data(vm, p, f)?;
    let mut od = ObjectData::new(Some(proto), Kind::RegExp(Box::new(d)));
    od.props.insert(PropertyKey::from_str("lastIndex"), Prop::data(Value::Number(0.0), W));
    Ok(vm.alloc(od))
}

/// Annex B.2.4.1 RegExp.prototype.compile(pattern, flags)
fn compile(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) if matches!(vm.heap.get(*o).kind, Kind::RegExp(_)) => *o,
        _ => return vm.throw_type("RegExp.prototype.compile called on incompatible receiver"),
    };
    let pattern = vm.arg(ctx, 0);
    let flags = vm.arg(ctx, 1);
    let (p, f) = if let Some((src, fl, _, _)) = data(vm, &pattern) {
        if !flags.is_undefined() {
            return vm.throw_type("Cannot supply flags when constructing one RegExp from another");
        }
        (Value::String(src), Value::String(fl))
    } else {
        (pattern, flags)
    };
    let d = parse_source(vm, &p, &f)?;
    if let Kind::RegExp(r) = &mut vm.heap.get_mut(o).kind {
        **r = d;
    }
    vm.set_prop(o, PropertyKey::from_str("lastIndex"), Value::Number(0.0), true)?;
    Ok(Value::Object(o))
}

// ------------------------------------------------------------------------------------------------ exec

fn advance(s: &[u16], i: usize, unicode: bool) -> usize {
    if !unicode || i + 1 >= s.len() {
        return i + 1;
    }
    i + crate::string::code_point_at(s, i).1
}

fn last_index(vm: &mut Vm, r: Obj) -> JsResult<f64> {
    let li = vm.get(r, &PropertyKey::from_str("lastIndex"))?;
    vm.to_length(&li)
}

fn set_last_index(vm: &mut Vm, r: Obj, v: f64) -> JsResult<()> {
    vm.set_prop(r, PropertyKey::from_str("lastIndex"), Value::Number(v), true)
}

/// RegExpBuiltinExec(R, S) (§22.2.7.2)
pub fn builtin_exec(vm: &mut Vm, r: Obj, s: &JsStr) -> JsResult<Value> {
    let len = s.len();
    let mut li = last_index(vm, r)?;
    let (_, _, fl, re) = data(vm, &Value::Object(r)).unwrap();
    let (global, sticky, has_indices) = (fl.g, fl.y, fl.d);
    let full_unicode = fl.unicode();
    if !global && !sticky {
        li = 0.0;
    }
    if li > len as f64 {
        if global || sticky {
            set_last_index(vm, r, 0.0)?;
        }
        return Ok(Value::Null);
    }
    let mut start = li as usize;
    let su = s.units();
    // A lastIndex inside a surrogate pair names the pair's character (Unicode mode).
    if full_unicode && start > 0 && start < len && (0xDC00..0xE000).contains(&su[start]) && (0xD800..0xDC00).contains(&su[start - 1]) {
        start -= 1;
    }
    let caps = match matcher::search(&re.prog, su, start, sticky, STEP_LIMIT) {
        MatchResult::Match(c) => c,
        MatchResult::NoMatch => {
            if global || sticky {
                set_last_index(vm, r, 0.0)?;
            }
            return Ok(Value::Null);
        }
        MatchResult::Aborted => return vm.throw_range("Maximum regular expression backtracking exceeded"),
    };
    let (ms, me) = (caps[0] as usize, caps[1] as usize);
    if global || sticky {
        set_last_index(vm, r, me as f64)?;
    }
    let n = re.regex.ngroups;
    let a = vm.array_create(0.0, None)?;
    vm.create_data_property_or_throw(a, PropertyKey::from_str("index"), Value::Number(ms as f64))?;
    vm.create_data_property_or_throw(a, PropertyKey::from_str("input"), Value::String(s.clone()))?;
    vm.create_data_property_or_throw(a, PropertyKey::from_f64(0.0), Value::String(s.slice(ms, me)))?;
    let names = &re.regex.names;
    let has_groups = !names.is_empty();
    let groups = if has_groups { Value::Object(vm.new_object(None)) } else { Value::Undefined };
    vm.create_data_property_or_throw(a, PropertyKey::from_str("groups"), groups.clone())?;
    let mut indices: Vec<Option<(usize, usize)>> = alloc::vec![Some((ms, me))];
    let mut group_names: Vec<Option<JsStr>> = Vec::new();
    let mut matched_names: Vec<&str> = Vec::new();
    for i in 1..=n {
        let (cs, ce) = (caps[2 * i], caps[2 * i + 1]);
        let v = if cs < 0 || ce < 0 {
            indices.push(None);
            Value::Undefined
        } else {
            indices.push(Some((cs as usize, ce as usize)));
            Value::String(s.slice(cs as usize, ce as usize))
        };
        vm.create_data_property_or_throw(a, PropertyKey::from_f64(i as f64), v.clone())?;
        if let Some((name, _)) = names.iter().find(|(_, idx)| *idx == i) {
            if matched_names.contains(&name.as_str()) {
                group_names.push(None);
            } else {
                if !v.is_undefined() {
                    matched_names.push(name.as_str());
                }
                let k = JsStr::from_str(name);
                vm.create_data_property_or_throw(groups.as_object().unwrap(), PropertyKey::from_js(k.clone()), v)?;
                group_names.push(Some(k));
            }
        } else {
            group_names.push(None);
        }
    }
    if has_indices {
        let ia = vm.array_create(0.0, None)?;
        let ig = if has_groups { Value::Object(vm.new_object(None)) } else { Value::Undefined };
        vm.create_data_property_or_throw(ia, PropertyKey::from_str("groups"), ig.clone())?;
        for (i, m) in indices.iter().enumerate() {
            let pair = match m {
                Some((x, y)) => Value::Object(vm.new_array(alloc::vec![Value::Number(*x as f64), Value::Number(*y as f64)])),
                None => Value::Undefined,
            };
            vm.create_data_property_or_throw(ia, PropertyKey::from_f64(i as f64), pair.clone())?;
            if i > 0 {
                if let Some(k) = &group_names[i - 1] {
                    vm.create_data_property_or_throw(ig.as_object().unwrap(), PropertyKey::from_js(k.clone()), pair)?;
                }
            }
        }
        vm.create_data_property_or_throw(a, PropertyKey::from_str("indices"), Value::Object(ia))?;
    }
    Ok(Value::Object(a))
}

/// RegExpExec(R, S) (§22.2.7.1)
pub fn regexp_exec(vm: &mut Vm, r: Obj, s: &JsStr) -> JsResult<Value> {
    let exec = vm.get(r, &PropertyKey::from_str("exec"))?;
    if vm.is_callable(&exec) {
        let res = vm.call(&exec, &Value::Object(r), &[Value::String(s.clone())])?;
        if !res.is_object() && !res.is_null() {
            return vm.throw_type("RegExp exec method returned something other than an Object or null");
        }
        vm.root(&res);
        return Ok(res);
    }
    if !matches!(vm.heap.get(r).kind, Kind::RegExp(_)) {
        return vm.throw_type("RegExp.prototype.exec called on incompatible receiver");
    }
    builtin_exec(vm, r, s)
}

fn this_regexp(vm: &mut Vm, ctx: &CallCtx, name: &str) -> JsResult<Obj> {
    match &ctx.this {
        Value::Object(o) if matches!(vm.heap.get(*o).kind, Kind::RegExp(_)) => Ok(*o),
        _ => vm.throw_type(&alloc::format!("RegExp.prototype.{} called on incompatible receiver", name)),
    }
}

fn this_object(vm: &mut Vm, ctx: &CallCtx, name: &str) -> JsResult<Obj> {
    match &ctx.this {
        Value::Object(o) => Ok(*o),
        _ => vm.throw_type(&alloc::format!("RegExp.prototype.{} called on non-object", name)),
    }
}

fn exec(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let r = this_regexp(vm, ctx, "exec")?;
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    builtin_exec(vm, r, &s)
}

fn test(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let r = this_object(vm, ctx, "test")?;
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let m = regexp_exec(vm, r, &s)?;
    Ok(Value::Bool(!m.is_null()))
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let r = this_object(vm, ctx, "toString")?;
    let src = vm.get(r, &PropertyKey::from_str("source"))?;
    let src = vm.to_string(&src)?;
    let fl = vm.get(r, &PropertyKey::from_str("flags"))?;
    let fl = vm.to_string(&fl)?;
    let mut out = alloc::vec![b'/' as u16];
    out.extend_from_slice(src.units());
    out.push(b'/' as u16);
    out.extend_from_slice(fl.units());
    Ok(Value::String(JsStr::from_units(out)))
}

// ------------------------------------------------------------------------------------------------ accessors

fn has_flag(vm: &mut Vm, ctx: &CallCtx, pick: fn(&Flags) -> bool, name: &str) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type(&alloc::format!("RegExp.prototype.{} getter called on non-object", name)),
    };
    if let Kind::RegExp(d) = &vm.heap.get(o).kind {
        return Ok(Value::Bool(pick(&d.parsed)));
    }
    if o == vm.intr().regexp_proto {
        return Ok(Value::Undefined);
    }
    vm.throw_type(&alloc::format!("RegExp.prototype.{} getter called on non-RegExp object", name))
}

fn get_dot_all(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.s, "dotAll")
}
fn get_global(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.g, "global")
}
fn get_has_indices(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.d, "hasIndices")
}
fn get_ignore_case(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.i, "ignoreCase")
}
fn get_multiline(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.m, "multiline")
}
fn get_sticky(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.y, "sticky")
}
fn get_unicode(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.u, "unicode")
}
fn get_unicode_sets(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    has_flag(vm, ctx, |f| f.v, "unicodeSets")
}

fn get_flags(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let r = this_object(vm, ctx, "flags")?;
    let mut out = Vec::new();
    for (name, ch) in [("hasIndices", 'd'), ("global", 'g'), ("ignoreCase", 'i'), ("multiline", 'm'), ("dotAll", 's'), ("unicode", 'u'), ("unicodeSets", 'v'), ("sticky", 'y')] {
        let v = vm.get(r, &PropertyKey::from_str(name))?;
        if vm.to_boolean(&v) {
            out.push(ch as u16);
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn get_source(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("RegExp.prototype.source getter called on non-object"),
    };
    if let Kind::RegExp(d) = &vm.heap.get(o).kind {
        return Ok(Value::String(escape_pattern(d.source.units())));
    }
    if o == vm.intr().regexp_proto {
        return Ok(Value::str("(?:)"));
    }
    vm.throw_type("RegExp.prototype.source getter called on non-RegExp object")
}

/// EscapeRegExpPattern (§22.2.6.13.1): a source text that re-parses as the same pattern inside `/…/`.
fn escape_pattern(src: &[u16]) -> JsStr {
    if src.is_empty() {
        return JsStr::from_str("(?:)");
    }
    let mut out = Vec::with_capacity(src.len());
    let mut in_class = false;
    let mut i = 0;
    while i < src.len() {
        let c = src[i];
        match c {
            0x5C => {
                out.push(c);
                if i + 1 < src.len() {
                    i += 1;
                    match src[i] {
                        0x0A => out.push(b'n' as u16),
                        0x0D => out.push(b'r' as u16),
                        0x2028 => out.extend("u2028".encode_utf16()),
                        0x2029 => out.extend("u2029".encode_utf16()),
                        d => out.push(d),
                    }
                }
            }
            0x2F if !in_class => out.extend("\\/".encode_utf16()),
            0x5B => {
                in_class = true;
                out.push(c);
            }
            0x5D => {
                in_class = false;
                out.push(c);
            }
            0x0A => out.extend("\\n".encode_utf16()),
            0x0D => out.extend("\\r".encode_utf16()),
            0x2028 => out.extend("\\u2028".encode_utf16()),
            0x2029 => out.extend("\\u2029".encode_utf16()),
            _ => out.push(c),
        }
        i += 1;
    }
    JsStr::from_units(out)
}

// ------------------------------------------------------------------------------------------------ protocol methods

fn flags_of(vm: &mut Vm, r: Obj) -> JsResult<JsStr> {
    let f = vm.get(r, &PropertyKey::from_str("flags"))?;
    vm.to_string(&f)
}

fn has_unit(s: &JsStr, c: char) -> bool {
    s.units().contains(&(c as u16))
}

fn match_str(vm: &mut Vm, result: &Value) -> JsResult<JsStr> {
    let m = vm.get_v(result, &PropertyKey::from_f64(0.0))?;
    vm.to_string(&m)
}

fn bump_if_empty(vm: &mut Vm, r: Obj, s: &JsStr, m: &JsStr, full_unicode: bool) -> JsResult<()> {
    if m.is_empty() {
        let this_index = last_index(vm, r)?;
        let next = if this_index >= s.len() as f64 || !full_unicode { this_index + 1.0 } else { advance(s.units(), this_index as usize, true) as f64 };
        set_last_index(vm, r, next)?;
    }
    Ok(())
}

fn sym_match(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let rx = this_object(vm, ctx, "[Symbol.match]")?;
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let flags = flags_of(vm, rx)?;
    if !has_unit(&flags, 'g') {
        return regexp_exec(vm, rx, &s);
    }
    let full_unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
    set_last_index(vm, rx, 0.0)?;
    let arr = vm.array_create(0.0, None)?;
    let mut n = 0u32;
    loop {
        let result = regexp_exec(vm, rx, &s)?;
        if result.is_null() {
            return Ok(if n == 0 { Value::Null } else { Value::Object(arr) });
        }
        let m = match_str(vm, &result)?;
        vm.create_data_property_or_throw(arr, PropertyKey::from_f64(n as f64), Value::String(m.clone()))?;
        bump_if_empty(vm, rx, &s, &m, full_unicode)?;
        n += 1;
    }
}

fn sym_match_all(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let r = this_object(vm, ctx, "[Symbol.matchAll]")?;
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let dc = vm.intr().regexp_ctor;
    let c = vm.species_constructor(r, dc)?;
    let flags = flags_of(vm, r)?;
    let matcher = vm.construct(&c, &[Value::Object(r), Value::String(flags.clone())], None)?;
    vm.root(&matcher);
    let li = last_index(vm, r)?;
    let mo = matcher.as_object().unwrap();
    set_last_index(vm, mo, li)?;
    let global = has_unit(&flags, 'g');
    let unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
    let p = vm.intr().regexp_string_iterator_proto;
    let it = vm.alloc(ObjectData::new(Some(p), Kind::Iterator(Box::new(IterData::RegExpString { regexp: mo, s, global, unicode, done: false }))));
    Ok(Value::Object(it))
}

fn rsi_next(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) => *o,
        _ => return vm.throw_type("%RegExpStringIteratorPrototype%.next called on incompatible receiver"),
    };
    let (r, s, global, unicode, done) = match &vm.heap.get(o).kind {
        Kind::Iterator(d) => match &**d {
            IterData::RegExpString { regexp, s, global, unicode, done } => (*regexp, s.clone(), *global, *unicode, *done),
            _ => return vm.throw_type("%RegExpStringIteratorPrototype%.next called on incompatible receiver"),
        },
        _ => return vm.throw_type("%RegExpStringIteratorPrototype%.next called on incompatible receiver"),
    };
    let set_done = |vm: &mut Vm| {
        if let Kind::Iterator(d) = &mut vm.heap.get_mut(o).kind {
            if let IterData::RegExpString { done, .. } = &mut **d {
                *done = true;
            }
        }
    };
    if done {
        return Ok(Value::Object(vm.iter_result(Value::Undefined, true)));
    }
    let m = regexp_exec(vm, r, &s)?;
    if m.is_null() {
        set_done(vm);
        return Ok(Value::Object(vm.iter_result(Value::Undefined, true)));
    }
    if global {
        let ms = match_str(vm, &m)?;
        bump_if_empty(vm, r, &s, &ms, unicode)?;
    } else {
        set_done(vm);
    }
    Ok(Value::Object(vm.iter_result(m, false)))
}

fn sym_replace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let rx = this_object(vm, ctx, "[Symbol.replace]")?;
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let len_s = s.len();
    let mut replace_value = vm.arg(ctx, 1);
    let functional = vm.is_callable(&replace_value);
    if !functional {
        replace_value = Value::String(vm.to_string(&replace_value)?);
    }
    let flags = flags_of(vm, rx)?;
    let global = has_unit(&flags, 'g');
    let mut full_unicode = false;
    if global {
        full_unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
        set_last_index(vm, rx, 0.0)?;
    }
    let mut results = Vec::new();
    loop {
        let result = regexp_exec(vm, rx, &s)?;
        if result.is_null() {
            break;
        }
        vm.root(&result);
        results.push(result.clone());
        if !global {
            break;
        }
        let m = match_str(vm, &result)?;
        bump_if_empty(vm, rx, &s, &m, full_unicode)?;
    }
    let mut acc: Vec<u16> = Vec::new();
    let mut next_pos = 0usize;
    for result in results {
        let ro = result.as_object().unwrap();
        let rl = vm.length_of(ro)?;
        let ncap = (rl - 1.0).max(0.0) as usize;
        let matched = match_str(vm, &result)?;
        let idx = vm.get(ro, &PropertyKey::from_str("index"))?;
        let pos = vm.to_integer_or_infinity(&idx)?;
        let position = pos.max(0.0).min(len_s as f64) as usize;
        let mut captures = Vec::with_capacity(ncap);
        for n in 1..=ncap {
            let c = vm.get(ro, &PropertyKey::from_f64(n as f64))?;
            let c = if c.is_undefined() { c } else { Value::String(vm.to_string(&c)?) };
            captures.push(c);
        }
        let named = vm.get(ro, &PropertyKey::from_str("groups"))?;
        let replacement = if functional {
            let mut args = Vec::with_capacity(ncap + 4);
            args.push(Value::String(matched.clone()));
            args.extend(captures.iter().cloned());
            args.push(Value::Number(position as f64));
            args.push(Value::String(s.clone()));
            if !named.is_undefined() {
                args.push(named);
            }
            let rv = vm.call(&replace_value, &Value::Undefined, &args)?;
            vm.to_string(&rv)?
        } else {
            let named = if named.is_undefined() { named } else { vm.to_object(&named)? };
            let tpl = match &replace_value {
                Value::String(t) => t.clone(),
                _ => unreachable!(),
            };
            super::string::get_substitution(vm, &matched, &s, position, &captures, &named, &tpl)?
        };
        if position >= next_pos {
            acc.extend_from_slice(&s.units()[next_pos..position]);
            acc.extend_from_slice(replacement.units());
            next_pos = position + matched.len();
            vm.check_string_len(acc.len())?;
        }
    }
    if next_pos < len_s {
        acc.extend_from_slice(&s.units()[next_pos..]);
    }
    Ok(Value::String(JsStr::from_units(acc)))
}

fn sym_search(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let rx = this_object(vm, ctx, "[Symbol.search]")?;
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let k = PropertyKey::from_str("lastIndex");
    let prev = vm.get(rx, &k)?;
    if !prev.same_value(&Value::Number(0.0)) {
        vm.set_prop(rx, k.clone(), Value::Number(0.0), true)?;
    }
    let result = regexp_exec(vm, rx, &s)?;
    let cur = vm.get(rx, &k)?;
    if !cur.same_value(&prev) {
        vm.set_prop(rx, k, prev, true)?;
    }
    if result.is_null() {
        return Ok(Value::Number(-1.0));
    }
    vm.get_v(&result, &PropertyKey::from_str("index"))
}

fn sym_split(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let rx = this_object(vm, ctx, "[Symbol.split]")?;
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let limit = vm.arg(ctx, 1);
    let dc = vm.intr().regexp_ctor;
    let c = vm.species_constructor(rx, dc)?;
    let flags = flags_of(vm, rx)?;
    let unicode = has_unit(&flags, 'u') || has_unit(&flags, 'v');
    let new_flags = if has_unit(&flags, 'y') { flags } else { flags.concat(&JsStr::from_str("y")) };
    let splitter = vm.construct(&c, &[Value::Object(rx), Value::String(new_flags)], None)?;
    vm.root(&splitter);
    let sp = splitter.as_object().unwrap();
    let arr = vm.array_create(0.0, None)?;
    let lim = if limit.is_undefined() { u32::MAX } else { vm.to_uint32(&limit)? };
    if lim == 0 {
        return Ok(Value::Object(arr));
    }
    let size = s.len();
    if size == 0 {
        let z = regexp_exec(vm, sp, &s)?;
        if !z.is_null() {
            return Ok(Value::Object(arr));
        }
        vm.create_data_property_or_throw(arr, PropertyKey::from_f64(0.0), Value::String(s))?;
        return Ok(Value::Object(arr));
    }
    let mut len_a = 0u32;
    let (mut p, mut q) = (0usize, 0usize);
    while q < size {
        set_last_index(vm, sp, q as f64)?;
        let z = regexp_exec(vm, sp, &s)?;
        if z.is_null() {
            q = advance(s.units(), q, unicode);
            continue;
        }
        let e = last_index(vm, sp)?;
        let e = (e as usize).min(size);
        if e == p {
            q = advance(s.units(), q, unicode);
            continue;
        }
        vm.create_data_property_or_throw(arr, PropertyKey::from_f64(len_a as f64), Value::String(s.slice(p, q)))?;
        len_a += 1;
        if len_a == lim {
            return Ok(Value::Object(arr));
        }
        p = e;
        let zo = z.as_object().unwrap();
        let nc = vm.length_of(zo)?;
        let nc = (nc - 1.0).max(0.0) as usize;
        for i in 1..=nc {
            let cap = vm.get(zo, &PropertyKey::from_f64(i as f64))?;
            vm.create_data_property_or_throw(arr, PropertyKey::from_f64(len_a as f64), cap)?;
            len_a += 1;
            if len_a == lim {
                return Ok(Value::Object(arr));
            }
        }
        q = p;
    }
    vm.create_data_property_or_throw(arr, PropertyKey::from_f64(len_a as f64), Value::String(s.slice(p, size)))?;
    Ok(Value::Object(arr))
}

// ------------------------------------------------------------------------------------------------ RegExp.escape

fn regexp_escape(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let s = match vm.arg(ctx, 0) {
        Value::String(s) => s,
        _ => return vm.throw_type("RegExp.escape requires a string"),
    };
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len() * 2);
    let mut i = 0;
    let hex = |out: &mut Vec<u16>, v: u32, w: usize, pre: &str| {
        out.extend(pre.encode_utf16());
        let digits = alloc::format!("{:0w$x}", v, w = w);
        out.extend(digits.encode_utf16());
    };
    while i < u.len() {
        let (c, n) = crate::string::code_point_at(u, i);
        if out.is_empty() && (matches!(c, 0x30..=0x39) || matches!(c, 0x41..=0x5A | 0x61..=0x7A)) {
            hex(&mut out, c, 2, "\\x");
        } else if matches!(char::from_u32(c), Some('^' | '$' | '\\' | '.' | '*' | '+' | '?' | '(' | ')' | '[' | ']' | '{' | '}' | '|' | '/')) {
            out.push(0x5C);
            out.push(c as u16);
        } else if let Some(e) = match c {
            0x09 => Some('t'),
            0x0A => Some('n'),
            0x0B => Some('v'),
            0x0C => Some('f'),
            0x0D => Some('r'),
            _ => None,
        } {
            out.push(0x5C);
            out.push(e as u16);
        } else if ",-=<>#&!%:;@~'`\"".chars().any(|p| p as u32 == c) || crate::unicode::is_js_whitespace(c) || crate::unicode::is_line_terminator(c) || (0xD800..0xE000).contains(&c) {
            if c <= 0xFF {
                hex(&mut out, c, 2, "\\x");
            } else {
                let mut tmp = Vec::new();
                crate::string::push_code_point(&mut tmp, c);
                for cu in tmp {
                    hex(&mut out, cu as u32, 4, "\\u");
                }
            }
        } else {
            out.extend_from_slice(&u[i..i + n]);
        }
        i += n;
    }
    Ok(Value::String(JsStr::from_units(out)))
}
