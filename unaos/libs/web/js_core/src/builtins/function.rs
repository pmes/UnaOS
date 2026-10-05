//! Function (§20.2), CreateDynamicFunction, bound functions, %ThrowTypeError%, and the GeneratorFunction /
//! AsyncFunction / AsyncGeneratorFunction constructors.

use super::*;

pub fn init(vm: &mut Vm) {
    let fp = vm.intr().function_proto;
    let c = ctor(vm, "Function", 1, function_ctor, fp);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.function_ctor = c;
    method(vm, fp, "apply", 2, apply);
    method(vm, fp, "bind", 1, bind);
    method(vm, fp, "call", 1, call);
    method(vm, fp, "toString", 0, to_string);
    let hi = vm.wk.has_instance.clone();
    method_sym(vm, fp, hi, "[Symbol.hasInstance]", 1, has_instance, 0);
    // %ThrowTypeError%
    let tte = vm.make_native("", 0, throw_type_error, false);
    {
        let d = vm.heap.get_mut(tte);
        d.extensible = false;
        d.props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(0.0), 0));
        d.props.insert(PropertyKey::from_str("name"), Prop::data(Value::String(JsStr::empty()), 0));
    }
    vm.realms[r].intrinsics.throw_type_error = tte;
    for n in ["caller", "arguments"] {
        vm.heap.get_mut(fp).props.insert(PropertyKey::from_str(n), Prop { slot: Slot::Accessor(Some(tte), Some(tte)), flags: C });
    }
    global(vm, "Function", Value::Object(c));
    // GeneratorFunction, AsyncFunction, AsyncGeneratorFunction: prototypes inherit from %Function.prototype%.
    for (name, kind) in [("GeneratorFunction", 1u8), ("AsyncFunction", 2), ("AsyncGeneratorFunction", 3)] {
        let proto = vm.new_object(Some(fp));
        let cf = vm.make_native_with(name, 1, match kind {
            1 => generator_function_ctor as NativeFn,
            2 => async_function_ctor,
            _ => async_generator_function_ctor,
        }, true, Some(vm.realms[r].intrinsics.function_ctor), Vec::new());
        vm.heap.get_mut(cf).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(proto), 0));
        vm.heap.get_mut(proto).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(cf), C));
        to_str_tag(vm, proto, name);
        let i = &mut vm.realms[r].intrinsics;
        match kind {
            1 => {
                i.generator_function_proto = proto;
                i.generator_function_ctor = cf;
            }
            2 => {
                i.async_function_proto = proto;
                i.async_function_ctor = cf;
            }
            _ => {
                i.async_generator_function_proto = proto;
                i.async_generator_function_ctor = cf;
            }
        }
    }
}

fn throw_type_error(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    vm.throw_type("'caller', 'callee', and 'arguments' properties may not be accessed on strict mode functions or the arguments objects for calls to them")
}

fn function_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    create_dynamic_function(vm, ctx, 0)
}
fn generator_function_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    create_dynamic_function(vm, ctx, 1)
}
fn async_function_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    create_dynamic_function(vm, ctx, 2)
}
fn async_generator_function_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    create_dynamic_function(vm, ctx, 3)
}

/// CreateDynamicFunction (§20.2.1.1.1). kind: 0 normal, 1 generator, 2 async, 3 async generator.
pub fn create_dynamic_function(vm: &mut Vm, ctx: &CallCtx, kind: u8) -> JsResult<Value> {
    let n = ctx.argc;
    let mut params: Vec<u16> = Vec::new();
    let body = if n == 0 {
        JsStr::empty()
    } else {
        for i in 0..n - 1 {
            let a = vm.arg(ctx, i);
            let s = vm.to_string(&a)?;
            if i > 0 {
                params.push(b',' as u16);
            }
            params.extend_from_slice(s.units());
        }
        let b = vm.arg(ctx, n - 1);
        vm.to_string(&b)?
    };
    let prefix = match kind {
        1 => "function*",
        2 => "async function",
        3 => "async function*",
        _ => "function",
    };
    let mut src: Vec<u16> = Vec::new();
    src.extend(prefix.encode_utf16());
    src.extend(" anonymous(".encode_utf16());
    src.extend_from_slice(&params);
    src.extend("\n) {\n".encode_utf16());
    src.extend_from_slice(body.units());
    src.extend("\n}".encode_utf16());
    let mut wrapped: Vec<u16> = Vec::with_capacity(src.len() + 2);
    wrapped.push(b'(' as u16);
    wrapped.extend_from_slice(&src);
    wrapped.push(b')' as u16);
    let prog = match crate::parser::parse_script(&wrapped) {
        Ok(p) => p,
        Err(e) => return vm.throw_syntax(&e.msg),
    };
    // The text must be exactly one parenthesised function expression spanning the synthesised source.
    let f = match prog.body.as_slice() {
        [crate::ast::Stmt::Expr(e, _)] => match &**e {
            crate::ast::Expr::Paren(inner, _) => match &**inner {
                crate::ast::Expr::Function(f) if f.span.start == 1 && f.span.end as usize == src.len() + 1 => f.clone(),
                _ => return vm.throw_syntax("Invalid function source"),
            },
            _ => return vm.throw_syntax("Invalid function source"),
        },
        _ => return vm.throw_syntax("Invalid function source"),
    };
    let _ = f;
    let code = crate::compiler::compile_script(&prog);
    // Run the wrapper script in the realm's global environment to obtain the closure.
    let r = vm.cur_realm;
    let genv = vm.realms[r as usize].global_env;
    let script_code = code;
    let base = vm.stack.len();
    vm.stack.push(Value::Undefined);
    vm.stack.push(vm.realms[r as usize].global_this.clone());
    let nl = script_code.nlocals as usize;
    vm.stack.resize(base + 2 + nl, Value::Undefined);
    let hb = vm.handlers.len();
    let gt = vm.realms[r as usize].global_this.clone();
    vm.frames.push(Frame {
        code: script_code,
        pc: 0,
        args_base: base + 2,
        argc: 0,
        base: base + 2,
        func: None,
        this: gt,
        new_target: Value::Undefined,
        env: Some(genv),
        handler_base: hb,
        realm: r,
        construct: false,
        entry: true,
        coroutine: None,
        resume_kind: 0,
        script: None,
    });
    let saved_native = vm.in_native;
    vm.in_native = false;
    let res = vm.run();
    vm.in_native = saved_native;
    vm.stack.truncate(base);
    let fo = match res? {
        crate::vm::interp::Completion::Return(Value::Object(o)) => o,
        _ => return vm.throw_type("CreateDynamicFunction failed"),
    };
    vm.root(&Value::Object(fo));
    // Source text excludes the wrapping parentheses.
    if let Kind::Function(fd) = &mut vm.heap.get_mut(fo).kind {
        let mut c = (*fd.code).clone_shallow();
        c.source = Some(crate::bytecode::SourceRef { src: Rc::from(src.as_slice()), start: 0, end: src.len() as u32 });
        fd.code = Rc::new(c);
    }
    // Prototype from newTarget.
    let nt = if ctx.new_target.is_undefined() { Value::Object(ctx.callee) } else { ctx.new_target.clone() };
    let fallback = move |i: &Intrinsics| match kind {
        1 => i.generator_function_proto,
        2 => i.async_function_proto,
        3 => i.async_generator_function_proto,
        _ => i.function_proto,
    };
    let proto = vm.get_prototype_from_ctor(&nt, fallback)?;
    vm.heap.get_mut(fo).proto = Some(proto);
    Ok(Value::Object(fo))
}

fn apply(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let f = ctx.this.clone();
    if !vm.is_callable(&f) {
        return vm.throw_type("Function.prototype.apply was called on a non-function");
    }
    let this = vm.arg(ctx, 0);
    let a = vm.arg(ctx, 1);
    let args = if a.is_nullish() { Vec::new() } else { vm.list_from_array_like(&a)? };
    vm.call(&f, &this, &args)
}

fn call(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let f = ctx.this.clone();
    if !vm.is_callable(&f) {
        return vm.throw_type("Function.prototype.call was called on a non-function");
    }
    let this = vm.arg(ctx, 0);
    let args: Vec<Value> = if ctx.argc > 1 { vm.stack[ctx.args_base + 1..ctx.args_base + ctx.argc].to_vec() } else { Vec::new() };
    vm.call(&f, &this, &args)
}

fn bind(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let target = match &ctx.this {
        Value::Object(o) if vm.obj_is_callable(*o) => *o,
        _ => return vm.throw_type("Bind must be called on a function"),
    };
    let this = vm.arg(ctx, 0);
    let args: Vec<Value> = if ctx.argc > 1 { vm.stack[ctx.args_base + 1..ctx.args_base + ctx.argc].to_vec() } else { Vec::new() };
    let nargs = args.len() as f64;
    let proto = vm.get_prototype_of(target)?;
    let b = vm.alloc(ObjectData::new(proto, Kind::Bound(Box::new(BoundData { target, this, args }))));
    let mut len = 0.0;
    if vm.has_own_property(target, &PropertyKey::from_str("length"))? {
        let l = vm.get(target, &PropertyKey::from_str("length"))?;
        if let Value::Number(n) = l {
            if n == f64::INFINITY {
                len = f64::INFINITY;
            } else if n != f64::NEG_INFINITY {
                let t = crate::vm::ops::integer_or_infinity(n);
                len = (t - nargs).max(0.0);
            }
        }
    }
    vm.heap.get_mut(b).props.insert(PropertyKey::from_str("length"), Prop::data(Value::Number(len), C));
    let n = vm.get(target, &PropertyKey::from_str("name"))?;
    let name = match n {
        Value::String(s) => s,
        _ => JsStr::empty(),
    };
    vm.heap.get_mut(b).props.insert(PropertyKey::from_str("name"), Prop::data(Value::String(JsStr::from_str("bound ").concat(&name)), C));
    Ok(Value::Object(b))
}

fn to_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = match &ctx.this {
        Value::Object(o) if vm.obj_is_callable(*o) => *o,
        _ => return vm.throw_type("Function.prototype.toString requires that 'this' be a Function"),
    };
    if let Kind::Function(fd) = &vm.heap.get(o).kind {
        if let Some(s) = &fd.code.source {
            return Ok(Value::String(JsStr::from_slice(&s.src[s.start as usize..s.end as usize])));
        }
    }
    let name = match &vm.heap.get(o).kind {
        Kind::Native(_) => match vm.heap.get(o).props.get(&PropertyKey::from_str("name")) {
            Some(Prop { slot: Slot::Data(Value::String(s)), .. }) => s.to_rust(),
            _ => alloc::string::String::new(),
        },
        _ => alloc::string::String::new(),
    };
    Ok(Value::String(JsStr::from_str(&alloc::format!("function {}() {{ [native code] }}", name))))
}

fn has_instance(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    let this = ctx.this.clone();
    Ok(Value::Bool(vm.ordinary_has_instance(&this, &v)?))
}

impl crate::bytecode::Code {
    /// Copy of the code with shared instruction / constant storage cloned.
    pub fn clone_shallow(&self) -> crate::bytecode::Code {
        crate::bytecode::Code {
            name: self.name.clone(),
            ops: self.ops.clone(),
            consts: self.consts.iter().map(|c| c.clone_const()).collect(),
            nlocals: self.nlocals,
            nparams: self.nparams,
            length: self.length,
            kind: self.kind,
            is_async: self.is_async,
            is_generator: self.is_generator,
            strict: self.strict,
            simple_params: self.simple_params,
            derived: self.derived,
            has_fields: self.has_fields,
            source: None,
            positions: self.positions.clone(),
            is_module: self.is_module,
            is_script: self.is_script,
            is_eval: self.is_eval,
            module: self.module.clone(),
        }
    }
}

impl crate::bytecode::Const {
    pub fn clone_const(&self) -> crate::bytecode::Const {
        use crate::bytecode::Const::*;
        match self {
            Num(n) => Num(*n),
            Str(s) => Str(s.clone()),
            BigInt(b) => BigInt(b.clone()),
            Code(c) => Code(c.clone()),
            Scope(s) => Scope(s.clone()),
            Template(t) => Template(t.clone()),
            Regex(a, b) => Regex(a.clone(), b.clone()),
            Decls(d) => Decls(d.clone()),
            Slots(s) => Slots(s.clone()),
        }
    }
}
