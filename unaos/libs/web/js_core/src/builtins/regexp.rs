//! RegExp (§22.2.4–§22.2.9).

use super::*;
use crate::regexp::RegExpData;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let proto = vm.new_object(Some(op));
    let c = ctor(vm, "RegExp", 2, regexp_ctor, proto);
    species_getter(vm, c);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.regexp_proto = proto;
    vm.realms[r].intrinsics.regexp_ctor = c;
    global(vm, "RegExp", Value::Object(c));
}

fn regexp_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let p = vm.arg(ctx, 0);
    let f = vm.arg(ctx, 1);
    Ok(Value::Object(regexp_create(vm, p, f)?))
}

/// RegExpCreate(P, F)
pub fn regexp_create(vm: &mut Vm, p: Value, f: Value) -> JsResult<Obj> {
    let src = if p.is_undefined() { JsStr::empty() } else { vm.to_string(&p)? };
    let flags = if f.is_undefined() { JsStr::empty() } else { vm.to_string(&f)? };
    regexp_alloc_init(vm, src, flags)
}

pub fn regexp_create_literal(vm: &mut Vm, p: JsStr, f: JsStr) -> JsResult<Obj> {
    regexp_alloc_init(vm, p, f)
}

fn regexp_alloc_init(vm: &mut Vm, src: JsStr, flags: JsStr) -> JsResult<Obj> {
    let fl = match crate::regexp::parser::Flags::parse(flags.units()) {
        Ok(f) => f,
        Err(m) => return vm.throw_syntax(&alloc::format!("Invalid regular expression flags: {}", m)),
    };
    let rx = match crate::regexp::parser::parse(src.units(), fl) {
        Ok(r) => r,
        Err(m) => return vm.throw_syntax(&alloc::format!("Invalid regular expression: /{}/: {}", src, m)),
    };
    let p = vm.intr().regexp_proto;
    let mut d = ObjectData::new(Some(p), Kind::RegExp(Box::new(RegExpData { source: src, flags, regex: Some(Rc::new(rx)), last_index_cache: Value::Undefined })));
    d.props.insert(PropertyKey::from_str("lastIndex"), Prop::data(Value::Number(0.0), W));
    Ok(vm.alloc(d))
}
