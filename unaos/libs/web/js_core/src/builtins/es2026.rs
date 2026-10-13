//! The ECMAScript 2026 additions a current page reaches for, beside the ES2025 library: Array.fromAsync
//! (§23.1.2.2), Error.isError (§20.5.2.1), Math.sumPrecise (§21.3.2.34), the Uint8Array base64 / hex
//! methods (§23.3.x: fromBase64, fromHex, toBase64, toHex, setFromBase64, setFromHex), Map / WeakMap
//! getOrInsert / getOrInsertComputed (the upsert proposal, stage 4), Error.captureStackTrace (the
//! de-facto API every engine ships, which libraries call when present), and the library half of explicit
//! resource management (Symbol.dispose / asyncDispose, SuppressedError, DisposableStack,
//! AsyncDisposableStack, Iterator/AsyncIterator disposal), and the runtime half of the `using` /
//! `await using` declarations (USINGSYNTAX, SR114): the DisposeCapability the compiler keeps in a register
//! per scope and the `Using*` ops' AddDisposableResource / DisposeResources steps.

use super::*;
use crate::bignum::BigUint;
use crate::builtins::promise::{new_capability, perform_then, promise_resolve_intrinsic};
use alloc::string::String;
use alloc::vec;

pub fn init(vm: &mut Vm) {
    let ac = vm.intr().array_ctor;
    method(vm, ac, "fromAsync", 1, array_from_async);
    let ec = vm.intr().error_ctor;
    method(vm, ec, "isError", 1, error_is_error);
    method(vm, ec, "captureStackTrace", 1, error_capture_stack_trace);
    let m = vm.intr().math;
    method(vm, m, "sumPrecise", 1, math_sum_precise);
    let mp = vm.intr().map_proto;
    method(vm, mp, "getOrInsert", 2, map_get_or_insert);
    method(vm, mp, "getOrInsertComputed", 2, map_get_or_insert_computed);
    let wmp = vm.intr().weakmap_proto;
    method(vm, wmp, "getOrInsert", 2, weakmap_get_or_insert);
    method(vm, wmp, "getOrInsertComputed", 2, weakmap_get_or_insert_computed);
    let u8c = vm.intr().ta_ctors[1];
    let u8p = vm.intr().ta_protos[1];
    method(vm, u8c, "fromBase64", 1, u8_from_base64);
    method(vm, u8c, "fromHex", 1, u8_from_hex);
    method(vm, u8p, "toBase64", 0, u8_to_base64);
    method(vm, u8p, "toHex", 0, u8_to_hex);
    method(vm, u8p, "setFromBase64", 1, u8_set_from_base64);
    method(vm, u8p, "setFromHex", 1, u8_set_from_hex);
    init_dispose(vm);
}

// ============================================================================================ Error

fn error_is_error(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    Ok(Value::Bool(match v {
        Value::Object(o) => matches!(vm.heap.get(o).kind, Kind::Error(_)),
        _ => false,
    }))
}

/// Error.captureStackTrace(target[, constructorOpt]): an own `stack` string "Name: message\n    at …".
fn error_capture_stack_trace(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = vm.arg(ctx, 0);
    let o = match t {
        Value::Object(o) => o,
        _ => return vm.throw_type("Invalid argument"),
    };
    let name = vm.get(o, &PropertyKey::from_str("name"))?;
    let name = if name.is_undefined() { JsStr::from_str("Error") } else { vm.to_string(&name)? };
    let msg = vm.get(o, &PropertyKey::from_str("message"))?;
    let msg = if msg.is_undefined() { JsStr::empty() } else { vm.to_string(&msg)? };
    let mut head = name.to_rust();
    if !msg.is_empty() {
        if !head.is_empty() {
            head.push_str(": ");
        }
        head.push_str(&msg.to_rust());
    }
    let trace = vm.stack_trace().to_rust();
    if !trace.is_empty() {
        head.push('\n');
        head.push_str(&trace);
    }
    let d = PropDesc::data(Value::str(&head), true, false, true);
    vm.define_property_or_throw(o, PropertyKey::from_str("stack"), d)?;
    Ok(Value::Undefined)
}

// ============================================================================================ Math.sumPrecise

/// A signed big integer in units of 2^-1074 (every finite double is an integer multiple of it).
struct Fixed {
    neg: bool,
    mag: BigUint,
}

impl Fixed {
    fn add_f64(&mut self, x: f64) {
        let bits = x.to_bits();
        let neg = bits >> 63 == 1;
        let be = ((bits >> 52) & 0x7ff) as i64;
        let mut m = bits & ((1u64 << 52) - 1);
        let shift = if be == 0 {
            0
        } else {
            m |= 1u64 << 52;
            be - 1
        };
        if m == 0 {
            return;
        }
        let v = BigUint::from_u64(m).shl(shift as u64);
        if neg == self.neg {
            self.mag = self.mag.add(&v);
        } else {
            match self.mag.cmp(&v) {
                core::cmp::Ordering::Less => {
                    self.mag = v.sub(&self.mag);
                    self.neg = neg;
                }
                _ => self.mag = self.mag.sub(&v),
            }
        }
    }
}

fn math_sum_precise(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let items = vm.arg(ctx, 0);
    if items.is_nullish() {
        return vm.throw_type("Math.sumPrecise called on null or undefined");
    }
    let (it, next) = vm.get_iterator(&items)?;
    // state: 0 minus-zero, 1 finite, 2 +inf, 3 -inf, 4 NaN
    let mut state = 0u8;
    let mut sum = Fixed { neg: false, mag: BigUint::zero() };
    let mut count: f64 = 0.0;
    while let Some(v) = vm.iterator_step_value(&it, &next)? {
        vm.tick()?;
        count += 1.0;
        if count >= 9007199254740992.0 {
            let e = vm.range_error("Math.sumPrecise: too many values");
            let _ = vm.iterator_close(&it);
            return Err(e);
        }
        let n = match v {
            Value::Number(n) => n,
            _ => {
                let e = vm.type_error("Math.sumPrecise: value is not a Number");
                let _ = vm.iterator_close(&it);
                return Err(e);
            }
        };
        if state == 4 {
            continue;
        }
        if n.is_nan() {
            state = 4;
        } else if n == f64::INFINITY {
            state = if state == 3 { 4 } else { 2 };
        } else if n == f64::NEG_INFINITY {
            state = if state == 2 { 4 } else { 3 };
        } else if !(n == 0.0 && n.is_sign_negative()) && (state == 0 || state == 1) {
            state = 1;
            sum.add_f64(n);
        }
    }
    Ok(Value::Number(match state {
        0 => -0.0,
        2 => f64::INFINITY,
        3 => f64::NEG_INFINITY,
        4 => f64::NAN,
        _ => {
            if sum.mag.is_zero() {
                0.0
            } else {
                let r = crate::numconv::round_to_f64(&sum.mag, false, -1074);
                if sum.neg { -r } else { r }
            }
        }
    }))
}

// ============================================================================================ Map upsert

fn canon_key(k: Value) -> Value {
    match k {
        Value::Number(n) if n == 0.0 => Value::Number(0.0),
        other => other,
    }
}

fn this_kind_map(vm: &mut Vm, v: &Value, weak: bool) -> JsResult<Obj> {
    if let Value::Object(o) = v {
        match (&vm.heap.get(*o).kind, weak) {
            (Kind::Map(_), false) | (Kind::WeakMap(_), true) => return Ok(*o),
            _ => {}
        }
    }
    vm.throw_type(if weak { "WeakMap method called on incompatible receiver" } else { "Map method called on incompatible receiver" })
}

fn get_or_insert(vm: &mut Vm, ctx: &CallCtx, weak: bool) -> JsResult<Value> {
    let m = this_kind_map(vm, &ctx.this, weak)?;
    let k = canon_key(vm.arg(ctx, 0));
    if weak && !crate::builtins::weak::can_be_held_weakly(&k) {
        return vm.throw_type("Invalid value used as weak map key");
    }
    if let Some(v) = crate::builtins::map::map_data_ref(vm, m).get(&k) {
        return Ok(v.clone());
    }
    let v = vm.arg(ctx, 1);
    crate::builtins::map::map_data(vm, m).set(k, v.clone());
    Ok(v)
}

fn get_or_insert_computed(vm: &mut Vm, ctx: &CallCtx, weak: bool) -> JsResult<Value> {
    let m = this_kind_map(vm, &ctx.this, weak)?;
    let cb = vm.arg(ctx, 1);
    if !vm.is_callable(&cb) {
        return vm.throw_type("callbackfn is not a function");
    }
    let k = canon_key(vm.arg(ctx, 0));
    if weak && !crate::builtins::weak::can_be_held_weakly(&k) {
        return vm.throw_type("Invalid value used as weak map key");
    }
    if let Some(v) = crate::builtins::map::map_data_ref(vm, m).get(&k) {
        return Ok(v.clone());
    }
    let v = vm.call(&cb, &Value::Undefined, &[k.clone()])?;
    // The callback may have mutated the map: set overwrites an entry it added.
    crate::builtins::map::map_data(vm, m).set(k, v.clone());
    Ok(v)
}

fn map_get_or_insert(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    get_or_insert(vm, ctx, false)
}
fn map_get_or_insert_computed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    get_or_insert_computed(vm, ctx, false)
}
fn weakmap_get_or_insert(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    get_or_insert(vm, ctx, true)
}
fn weakmap_get_or_insert_computed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    get_or_insert_computed(vm, ctx, true)
}

// ============================================================================================ Array.fromAsync

// State record slots.
const S_A: usize = 0;
const S_ITER: usize = 1;
const S_NEXT: usize = 2;
const S_MAP: usize = 3;
const S_THIS: usize = 4;
const S_K: usize = 5;
const S_RES: usize = 6;
const S_REJ: usize = 7;
const S_MODE: usize = 8;
const S_SRC: usize = 9;
const S_LEN: usize = 10;

fn st_get(vm: &Vm, st: Obj, i: usize) -> Value {
    match &vm.heap.get(st).kind {
        Kind::Internal(v) => v[i].clone(),
        _ => Value::Undefined,
    }
}
fn st_set(vm: &mut Vm, st: Obj, i: usize, x: Value) {
    if let Kind::Internal(v) = &mut vm.heap.get_mut(st).kind {
        v[i] = x;
    }
}

fn fa_reject(vm: &mut Vm, st: Obj, e: Value) {
    let rej = st_get(vm, st, S_REJ);
    let _ = vm.call(&rej, &Value::Undefined, &[e]);
}

fn fa_resolve(vm: &mut Vm, st: Obj, v: Value) {
    let res = st_get(vm, st, S_RES);
    let _ = vm.call(&res, &Value::Undefined, &[v]);
}

/// Await(v) inside the closure: continue with `ok(state, value)` or `err(state, reason)`.
fn fa_await(vm: &mut Vm, st: Obj, v: Value, ok: NativeFn, err: NativeFn) {
    match promise_resolve_intrinsic(vm, v) {
        Ok(p) => {
            let fo = vm.make_native_closure("", 1, ok, vec![Value::Object(st)]);
            let fe = vm.make_native_closure("", 1, err, vec![Value::Object(st)]);
            perform_then(vm, p, Value::Object(fo), Value::Object(fe), None);
        }
        Err(e) => fa_on_error(vm, st, e),
    }
}

/// An abrupt completion in the iterator path closes the async iterator (awaiting `return()`), then rejects.
fn fa_on_error(vm: &mut Vm, st: Obj, e: Value) {
    fa_reject(vm, st, e);
}

fn fa_close_and_reject(vm: &mut Vm, st: Obj, e: Value) {
    let it = st_get(vm, st, S_ITER);
    if let Ok(Some(ret)) = vm.get_method(&it, &PropertyKey::from_str("return")) {
        if let Ok(r) = vm.call(&ret, &it, &[]) {
            // AsyncIteratorClose with a throw completion: await the result, then report the original error.
            st_set(vm, st, S_LEN, e.clone());
            fa_await(vm, st, r, fa_closed, fa_closed);
            return;
        }
    }
    fa_reject(vm, st, e);
}

fn fa_closed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let e = st_get(vm, st, S_LEN);
    fa_reject(vm, st, e);
    Ok(Value::Undefined)
}

fn fa_rejected(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let e = vm.arg(ctx, 0);
    fa_reject(vm, st, e);
    Ok(Value::Undefined)
}

fn fa_rejected_close(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let e = vm.arg(ctx, 0);
    fa_close_and_reject(vm, st, e);
    Ok(Value::Undefined)
}

fn k_of(vm: &Vm, st: Obj) -> f64 {
    match st_get(vm, st, S_K) {
        Value::Number(n) => n,
        _ => 0.0,
    }
}

/// Run the closure until it awaits.
fn fa_step(vm: &mut Vm, st: Obj) {
    let k = k_of(vm, st);
    if matches!(st_get(vm, st, S_MODE), Value::Number(m) if m == 0.0) {
        if k >= 9007199254740991.0 {
            let e = vm.type_error("Array.fromAsync: too many elements");
            fa_close_and_reject(vm, st, e);
            return;
        }
        let it = st_get(vm, st, S_ITER);
        let next = st_get(vm, st, S_NEXT);
        match vm.call(&next, &it, &[]) {
            Ok(r) => fa_await(vm, st, r, fa_iter_result, fa_rejected),
            Err(e) => fa_reject(vm, st, e),
        }
    } else {
        let len = match st_get(vm, st, S_LEN) {
            Value::Number(n) => n,
            _ => 0.0,
        };
        if k >= len {
            let a = st_get(vm, st, S_A).as_object().unwrap();
            match vm.set_prop(a, PropertyKey::from_str("length"), Value::Number(len), true) {
                Ok(()) => fa_resolve(vm, st, Value::Object(a)),
                Err(e) => fa_reject(vm, st, e),
            }
            return;
        }
        let src = st_get(vm, st, S_SRC).as_object().unwrap();
        match vm.get(src, &PropertyKey::from_f64(k)) {
            Ok(v) => fa_await(vm, st, v, fa_value, fa_rejected),
            Err(e) => fa_reject(vm, st, e),
        }
    }
}

fn fa_iter_result(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let r = vm.arg(ctx, 0);
    if !r.is_object() {
        let e = vm.type_error("iterator result is not an object");
        fa_reject(vm, st, e);
        return Ok(Value::Undefined);
    }
    let done = match vm.get_v(&r, &PropertyKey::from_str("done")) {
        Ok(d) => vm.to_boolean(&d),
        Err(e) => {
            fa_reject(vm, st, e);
            return Ok(Value::Undefined);
        }
    };
    if done {
        let a = st_get(vm, st, S_A).as_object().unwrap();
        let k = k_of(vm, st);
        match vm.set_prop(a, PropertyKey::from_str("length"), Value::Number(k), true) {
            Ok(()) => fa_resolve(vm, st, Value::Object(a)),
            Err(e) => fa_reject(vm, st, e),
        }
        return Ok(Value::Undefined);
    }
    let v = match vm.get_v(&r, &PropertyKey::from_str("value")) {
        Ok(v) => v,
        Err(e) => {
            fa_reject(vm, st, e);
            return Ok(Value::Undefined);
        }
    };
    fa_value_inner(vm, st, v, true);
    Ok(Value::Undefined)
}

fn fa_value(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let v = vm.arg(ctx, 0);
    fa_value_inner(vm, st, v, false);
    Ok(Value::Undefined)
}

fn fa_value_inner(vm: &mut Vm, st: Obj, v: Value, iter: bool) {
    let mapfn = st_get(vm, st, S_MAP);
    if mapfn.is_undefined() {
        fa_define(vm, st, v, iter);
        return;
    }
    let this = st_get(vm, st, S_THIS);
    let k = k_of(vm, st);
    match vm.call(&mapfn, &this, &[v, Value::Number(k)]) {
        Ok(m) => {
            if iter {
                fa_await(vm, st, m, fa_mapped_iter, fa_rejected_close)
            } else {
                fa_await(vm, st, m, fa_mapped, fa_rejected)
            }
        }
        Err(e) => {
            if iter {
                fa_close_and_reject(vm, st, e)
            } else {
                fa_reject(vm, st, e)
            }
        }
    }
}

fn fa_mapped_iter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let v = vm.arg(ctx, 0);
    fa_define(vm, st, v, true);
    Ok(Value::Undefined)
}

fn fa_mapped(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let v = vm.arg(ctx, 0);
    fa_define(vm, st, v, false);
    Ok(Value::Undefined)
}

fn fa_define(vm: &mut Vm, st: Obj, v: Value, iter: bool) {
    let a = st_get(vm, st, S_A).as_object().unwrap();
    let k = k_of(vm, st);
    if let Err(e) = vm.create_data_property_or_throw(a, PropertyKey::from_f64(k), v) {
        if iter {
            fa_close_and_reject(vm, st, e);
        } else {
            fa_reject(vm, st, e);
        }
        return;
    }
    st_set(vm, st, S_K, Value::Number(k + 1.0));
    fa_step(vm, st);
}

fn array_from_async(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let pc = Value::Object(vm.intr().promise_ctor);
    let (p, res, rej) = new_capability(vm, &pc)?;
    let c = ctx.this.clone();
    let items = vm.arg(ctx, 0);
    let mapfn = vm.arg(ctx, 1);
    let this_arg = vm.arg(ctx, 2);
    let start = (|| -> JsResult<Obj> {
        if !mapfn.is_undefined() && !vm.is_callable(&mapfn) {
            return vm.throw_type("Array.fromAsync: mapper is not a function");
        }
        let ak = PropertyKey::Sym(vm.wk.async_iterator.clone());
        let ik = PropertyKey::Sym(vm.wk.iterator.clone());
        let using_async = vm.get_method(&items, &ak)?;
        let using_sync = if using_async.is_none() { vm.get_method(&items, &ik)? } else { None };
        let st = vm.alloc(ObjectData::new(None, Kind::Internal(vec![Value::Undefined; 11])));
        st_set(vm, st, S_MAP, mapfn.clone());
        st_set(vm, st, S_THIS, this_arg.clone());
        st_set(vm, st, S_K, Value::Number(0.0));
        st_set(vm, st, S_RES, res.clone());
        st_set(vm, st, S_REJ, rej.clone());
        if using_async.is_some() || using_sync.is_some() {
            let (it, next) = match (using_async, using_sync) {
                (Some(m), _) => vm.get_iterator_from_method(&items, &m)?,
                (None, Some(m)) => {
                    let (it, next) = vm.get_iterator_from_method(&items, &m)?;
                    crate::builtins::generator::create_async_from_sync(vm, it, next)?
                }
                _ => unreachable!(),
            };
            let a = if vm.is_constructor(&c) { vm.construct(&c, &[], None)? } else { Value::Object(vm.array_create(0.0, None)?) };
            st_set(vm, st, S_A, a);
            st_set(vm, st, S_ITER, it);
            st_set(vm, st, S_NEXT, next);
            st_set(vm, st, S_MODE, Value::Number(0.0));
        } else {
            let src = vm.to_object(&items)?.as_object().unwrap();
            let len = vm.length_of(src)?;
            let a = if vm.is_constructor(&c) {
                vm.construct(&c, &[Value::Number(len)], None)?
            } else {
                Value::Object(vm.array_create(len, None)?)
            };
            st_set(vm, st, S_A, a);
            st_set(vm, st, S_SRC, Value::Object(src));
            st_set(vm, st, S_LEN, Value::Number(len));
            st_set(vm, st, S_MODE, Value::Number(1.0));
        }
        Ok(st)
    })();
    match start {
        Ok(st) => fa_step(vm, st),
        Err(e) => {
            let _ = vm.call(&rej, &Value::Undefined, &[e]);
        }
    }
    Ok(Value::Object(p))
}

// ============================================================================================ base64 / hex

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn b64_val(c: u8) -> Option<u8> {
    B64.iter().position(|&x| x == c).map(|p| p as u8)
}

fn is_ascii_ws(c: u16) -> bool {
    matches!(c, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// The decoded prefix, the code units read, and the error (a SyntaxError message) if decoding stopped on one.
struct Decoded {
    read: usize,
    bytes: Vec<u8>,
    error: Option<&'static str>,
}

fn decode_chunk(chunk: &[u8], strict: bool) -> Result<Vec<u8>, &'static str> {
    let n = chunk.len();
    let mut c = [0u8; 4];
    for (i, &x) in chunk.iter().enumerate() {
        c[i] = b64_val(x).unwrap_or(0);
    }
    let b = [(c[0] << 2) | (c[1] >> 4), (c[1] << 4) | (c[2] >> 2), (c[2] << 6) | c[3]];
    match n {
        2 => {
            if strict && b[1] != 0 {
                return Err("extra bits in the last base64 chunk");
            }
            Ok(vec![b[0]])
        }
        3 => {
            if strict && b[2] != 0 {
                return Err("extra bits in the last base64 chunk");
            }
            Ok(vec![b[0], b[1]])
        }
        _ => Ok(b.to_vec()),
    }
}

/// FromBase64 (the base64 proposal's decoding algorithm).
fn from_base64(s: &[u16], url: bool, last: &str, max_len: usize) -> Decoded {
    let mut out = Decoded { read: 0, bytes: Vec::new(), error: None };
    if max_len == 0 {
        return out;
    }
    let mut chunk: Vec<u8> = Vec::new();
    let mut i = 0;
    let len = s.len();
    let skip = |mut i: usize| {
        while i < len && is_ascii_ws(s[i]) {
            i += 1;
        }
        i
    };
    loop {
        i = skip(i);
        if i == len {
            if !chunk.is_empty() {
                if last == "stop-before-partial" {
                    return out;
                } else if last == "loose" {
                    if chunk.len() == 1 {
                        out.error = Some("incomplete base64 chunk");
                        return out;
                    }
                    match decode_chunk(&chunk, false) {
                        Ok(b) => out.bytes.extend(b),
                        Err(e) => {
                            out.error = Some(e);
                            return out;
                        }
                    }
                } else {
                    out.error = Some("incomplete base64 chunk");
                    return out;
                }
            }
            out.read = len;
            return out;
        }
        let mut ch = s[i];
        i += 1;
        if ch == b'=' as u16 {
            if chunk.len() < 2 {
                out.error = Some("unexpected padding");
                return out;
            }
            i = skip(i);
            if chunk.len() == 2 {
                if i == len {
                    if last == "stop-before-partial" {
                        return out;
                    }
                    out.error = Some("incomplete padding");
                    return out;
                }
                if s[i] == b'=' as u16 {
                    i = skip(i + 1);
                }
            }
            if i < len {
                out.error = Some("unexpected data after padding");
                return out;
            }
            match decode_chunk(&chunk, last == "strict") {
                Ok(b) => out.bytes.extend(b),
                Err(e) => {
                    out.error = Some(e);
                    return out;
                }
            }
            out.read = len;
            return out;
        }
        if url {
            if ch == b'+' as u16 || ch == b'/' as u16 {
                out.error = Some("invalid base64url character");
                return out;
            } else if ch == b'-' as u16 {
                ch = b'+' as u16;
            } else if ch == b'_' as u16 {
                ch = b'/' as u16;
            }
        }
        if ch > 0x7F || b64_val(ch as u8).is_none() {
            out.error = Some("invalid base64 character");
            return out;
        }
        let remaining = max_len - out.bytes.len();
        if (remaining == 1 && chunk.len() == 2) || (remaining == 2 && chunk.len() == 3) {
            return out;
        }
        chunk.push(ch as u8);
        if chunk.len() == 4 {
            out.bytes.extend(decode_chunk(&chunk, false).unwrap_or_default());
            chunk.clear();
            out.read = i;
            if out.bytes.len() == max_len {
                return out;
            }
        }
    }
}

fn from_hex(s: &[u16], max_len: usize) -> Decoded {
    let mut out = Decoded { read: 0, bytes: Vec::new(), error: None };
    if s.len() % 2 != 0 {
        out.error = Some("hex string length must be even");
        return out;
    }
    let hv = |c: u16| -> Option<u8> { if c < 0x80 { (c as u8 as char).to_digit(16).map(|d| d as u8) } else { None } };
    while out.read < s.len() && out.bytes.len() < max_len {
        match (hv(s[out.read]), hv(s[out.read + 1])) {
            (Some(a), Some(b)) => out.bytes.push(a * 16 + b),
            _ => {
                out.error = Some("invalid hex character");
                return out;
            }
        }
        out.read += 2;
    }
    out
}

fn b64_options(vm: &mut Vm, ov: &Value, with_last: bool) -> JsResult<(bool, String, bool)> {
    let o = match ov {
        Value::Undefined => None,
        Value::Object(o) => Some(*o),
        _ => return vm.throw_type("options must be an object"),
    };
    let get = |vm: &mut Vm, k: &str| -> JsResult<Value> {
        match o {
            Some(o) => vm.get(o, &PropertyKey::from_str(k)),
            None => Ok(Value::Undefined),
        }
    };
    let a = get(vm, "alphabet")?;
    let url = match &a {
        Value::Undefined => false,
        Value::String(s) if s.eq_str("base64") => false,
        Value::String(s) if s.eq_str("base64url") => true,
        _ => return vm.throw_type("alphabet must be \"base64\" or \"base64url\""),
    };
    let mut last = String::from("loose");
    let mut omit = false;
    if with_last {
        let l = get(vm, "lastChunkHandling")?;
        last = match &l {
            Value::Undefined => String::from("loose"),
            Value::String(s) if s.eq_str("loose") || s.eq_str("strict") || s.eq_str("stop-before-partial") => s.to_rust(),
            _ => return vm.throw_type("lastChunkHandling must be \"loose\", \"strict\" or \"stop-before-partial\""),
        };
    } else {
        let op = get(vm, "omitPadding")?;
        omit = vm.to_boolean(&op);
    }
    Ok((url, last, omit))
}

fn new_u8(vm: &mut Vm, bytes: &[u8]) -> JsResult<Value> {
    let c = Value::Object(vm.intr().ta_ctors[1]);
    let a = vm.construct(&c, &[Value::Number(bytes.len() as f64)], None)?;
    if let Value::Object(o) = a {
        write_bytes(vm, o, bytes);
    }
    Ok(a)
}

fn ta_bytes_range(vm: &Vm, o: Obj) -> Option<(Obj, usize, usize)> {
    let len = crate::builtins::typedarray::ta_length(vm, o)?;
    match &vm.heap.get(o).kind {
        Kind::TypedArray(t) => Some((t.buffer, t.byte_offset, len)),
        _ => None,
    }
}

fn write_bytes(vm: &mut Vm, o: Obj, bytes: &[u8]) {
    if let Some((buf, off, len)) = ta_bytes_range(vm, o) {
        if let Kind::ArrayBuffer(b) = &mut vm.heap.get_mut(buf).kind {
            let n = bytes.len().min(len);
            b.data[off..off + n].copy_from_slice(&bytes[..n]);
        }
    }
}

/// ValidateUint8Array + GetUint8ArrayBytes.
fn this_u8(vm: &mut Vm, v: &Value) -> JsResult<Obj> {
    if let Value::Object(o) = v {
        if let Kind::TypedArray(t) = &vm.heap.get(*o).kind {
            if crate::builtins::typedarray::kind_name(t.kind) == "Uint8Array" {
                return Ok(*o);
            }
        }
    }
    vm.throw_type("this is not a Uint8Array")
}

fn u8_bytes(vm: &mut Vm, o: Obj) -> JsResult<Vec<u8>> {
    match ta_bytes_range(vm, o) {
        Some((buf, off, len)) => match &vm.heap.get(buf).kind {
            Kind::ArrayBuffer(b) => Ok(b.data[off..off + len].to_vec()),
            _ => Ok(Vec::new()),
        },
        None => vm.throw_type("the Uint8Array is detached or out of bounds"),
    }
}

fn string_arg(vm: &mut Vm, v: &Value) -> JsResult<JsStr> {
    match v {
        Value::String(s) => Ok(s.clone()),
        _ => vm.throw_type("argument must be a string"),
    }
}

fn u8_from_base64(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let sv = vm.arg(ctx, 0);
    let s = string_arg(vm, &sv)?;
    let ov = vm.arg(ctx, 1);
    let (url, last, _) = b64_options(vm, &ov, true)?;
    let d = from_base64(s.units(), url, &last, usize::MAX);
    if let Some(e) = d.error {
        return vm.throw_syntax(e);
    }
    new_u8(vm, &d.bytes)
}

fn u8_from_hex(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let sv = vm.arg(ctx, 0);
    let s = string_arg(vm, &sv)?;
    let d = from_hex(s.units(), usize::MAX);
    if let Some(e) = d.error {
        return vm.throw_syntax(e);
    }
    new_u8(vm, &d.bytes)
}

fn u8_to_base64(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_u8(vm, &ctx.this)?;
    let ov = vm.arg(ctx, 0);
    let (url, _, omit) = b64_options(vm, &ov, false)?;
    let bytes = u8_bytes(vm, o)?;
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for ch in bytes.chunks(3) {
        let b = [ch[0], *ch.get(1).unwrap_or(&0), *ch.get(2).unwrap_or(&0)];
        let idx = [b[0] >> 2, ((b[0] & 3) << 4) | (b[1] >> 4), ((b[1] & 15) << 2) | (b[2] >> 6), b[2] & 63];
        for (i, &x) in idx.iter().enumerate() {
            if i <= ch.len() {
                let mut c = B64[x as usize];
                if url {
                    c = match c {
                        b'+' => b'-',
                        b'/' => b'_',
                        c => c,
                    };
                }
                out.push(c as char);
            } else if !omit {
                out.push('=');
            }
        }
    }
    Ok(Value::str(&out))
}

fn u8_to_hex(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_u8(vm, &ctx.this)?;
    let bytes = u8_bytes(vm, o)?;
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push_str(&alloc::format!("{b:02x}"));
    }
    Ok(Value::str(&out))
}

fn set_result(vm: &mut Vm, o: Obj, d: Decoded) -> JsResult<Value> {
    write_bytes(vm, o, &d.bytes);
    if let Some(e) = d.error {
        return vm.throw_syntax(e);
    }
    let r = vm.new_plain_object();
    vm.create_data_property_or_throw(r, PropertyKey::from_str("read"), Value::Number(d.read as f64))?;
    vm.create_data_property_or_throw(r, PropertyKey::from_str("written"), Value::Number(d.bytes.len() as f64))?;
    Ok(Value::Object(r))
}

fn u8_set_from_base64(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_u8(vm, &ctx.this)?;
    let sv = vm.arg(ctx, 0);
    let s = string_arg(vm, &sv)?;
    let ov = vm.arg(ctx, 1);
    let (url, last, _) = b64_options(vm, &ov, true)?;
    let max = match ta_bytes_range(vm, o) {
        Some((_, _, len)) => len,
        None => return vm.throw_type("the Uint8Array is detached or out of bounds"),
    };
    let d = from_base64(s.units(), url, &last, max);
    set_result(vm, o, d)
}

fn u8_set_from_hex(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_u8(vm, &ctx.this)?;
    let sv = vm.arg(ctx, 0);
    let s = string_arg(vm, &sv)?;
    let max = match ta_bytes_range(vm, o) {
        Some((_, _, len)) => len,
        None => return vm.throw_type("the Uint8Array is detached or out of bounds"),
    };
    let d = from_hex(s.units(), max);
    set_result(vm, o, d)
}

// ============================================================================================ explicit resource management

/// A DisposableStack's state: (value, method, hint) per resource — hint 0 `use` (call method on value),
/// 1 `adopt` (call onDispose(value)), 2 `defer` (call onDispose()), 3 an `@@dispose` used by an async stack
/// (called, its result not awaited).
pub struct DisposeData {
    pub disposed: bool,
    pub is_async: bool,
    pub res: Vec<(Value, Value, u8)>,
}

pub fn init_dispose(vm: &mut Vm) {
    let r = vm.cur_realm as usize;
    // SuppressedError (§20.5.8): an Error subclass carrying `error` and `suppressed`.
    let ep = vm.intr().error_proto;
    let ec = vm.intr().error_ctor;
    let sp = vm.new_object(Some(ep));
    let sc = vm.make_native_with("SuppressedError", 3, suppressed_error_ctor, true, Some(ec), Vec::new());
    vm.heap.get_mut(sc).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(sp), 0));
    vm.heap.get_mut(sp).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(sc), WC));
    value(vm, sp, "name", Value::str("SuppressedError"), WC);
    value(vm, sp, "message", Value::str(""), WC);
    vm.realms[r].intrinsics.suppressed_error_proto = sp;
    vm.realms[r].intrinsics.suppressed_error_ctor = sc;
    global(vm, "SuppressedError", Value::Object(sc));
    // DisposableStack (§27.3)
    let op = vm.intr().object_proto;
    let dp = vm.new_object(Some(op));
    let dc = ctor(vm, "DisposableStack", 0, ds_ctor, dp);
    method(vm, dp, "adopt", 2, ds_adopt);
    method(vm, dp, "defer", 1, ds_defer);
    let dispose = method(vm, dp, "dispose", 0, ds_dispose);
    method(vm, dp, "move", 0, ds_move);
    method(vm, dp, "use", 1, ds_use);
    accessor(vm, dp, PropertyKey::from_str("disposed"), "disposed", Some(ds_disposed), None, C);
    let dk = PropertyKey::Sym(vm.wk.dispose.clone());
    vm.heap.get_mut(dp).props.insert(dk.clone(), Prop::data(Value::Object(dispose), WC));
    to_str_tag(vm, dp, "DisposableStack");
    vm.realms[r].intrinsics.disposable_stack_proto = dp;
    global(vm, "DisposableStack", Value::Object(dc));
    // AsyncDisposableStack (§27.4)
    let ap = vm.new_object(Some(op));
    let acs = ctor(vm, "AsyncDisposableStack", 0, ads_ctor, ap);
    method(vm, ap, "adopt", 2, ds_adopt);
    method(vm, ap, "defer", 1, ds_defer);
    let dispose_async = method(vm, ap, "disposeAsync", 0, ads_dispose_async);
    method(vm, ap, "move", 0, ds_move);
    method(vm, ap, "use", 1, ds_use);
    accessor(vm, ap, PropertyKey::from_str("disposed"), "disposed", Some(ds_disposed), None, C);
    let adk = PropertyKey::Sym(vm.wk.async_dispose.clone());
    vm.heap.get_mut(ap).props.insert(adk.clone(), Prop::data(Value::Object(dispose_async), WC));
    to_str_tag(vm, ap, "AsyncDisposableStack");
    vm.realms[r].intrinsics.async_disposable_stack_proto = ap;
    global(vm, "AsyncDisposableStack", Value::Object(acs));
    // %IteratorPrototype%[@@dispose] and %AsyncIteratorPrototype%[@@asyncDispose] (§27.1.2.x)
    let ip = vm.intr().iterator_proto;
    let ds = vm.wk.dispose.clone();
    method_sym(vm, ip, ds, "[Symbol.dispose]", 0, iterator_dispose, WC);
    let aip = vm.intr().async_iterator_proto;
    let ads = vm.wk.async_dispose.clone();
    method_sym(vm, aip, ads, "[Symbol.asyncDispose]", 0, async_iterator_async_dispose, WC);
}

fn suppressed_error_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let nt = if ctx.new_target.is_undefined() { Value::Object(ctx.callee) } else { ctx.new_target.clone() };
    let proto = vm.get_prototype_from_ctor(&nt, |i| i.suppressed_error_proto)?;
    let trace = vm.stack_trace();
    let o = vm.alloc(ObjectData::new(Some(proto), Kind::Error(trace)));
    let msg = vm.arg(ctx, 2);
    if !msg.is_undefined() {
        let s = vm.to_string(&msg)?;
        vm.heap.get_mut(o).props.insert(PropertyKey::from_str("message"), Prop::data(Value::String(s), WC));
    }
    let e = vm.arg(ctx, 0);
    let s = vm.arg(ctx, 1);
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str("error"), Prop::data(e, WC));
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str("suppressed"), Prop::data(s, WC));
    Ok(Value::Object(o))
}

/// A SuppressedError for a disposal that threw `error` while `suppressed` was already pending.
fn suppressed(vm: &mut Vm, error: Value, suppressed: Value) -> Value {
    let proto = vm.intr().suppressed_error_proto;
    let trace = vm.stack_trace();
    let o = vm.alloc(ObjectData::new(Some(proto), Kind::Error(trace)));
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str("error"), Prop::data(error, WC));
    vm.heap.get_mut(o).props.insert(PropertyKey::from_str("suppressed"), Prop::data(suppressed, WC));
    Value::Object(o)
}

fn new_stack(vm: &mut Vm, proto: Obj, is_async: bool, res: Vec<(Value, Value, u8)>) -> Obj {
    vm.alloc(ObjectData::new(Some(proto), Kind::Disposable(Box::new(DisposeData { disposed: false, is_async, res }))))
}

fn ds_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor DisposableStack requires 'new'");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.disposable_stack_proto)?;
    Ok(Value::Object(new_stack(vm, p, false, Vec::new())))
}

fn ads_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor AsyncDisposableStack requires 'new'");
    }
    let p = vm.get_prototype_from_ctor(&ctx.new_target, |i| i.async_disposable_stack_proto)?;
    Ok(Value::Object(new_stack(vm, p, true, Vec::new())))
}

/// The stack behind `this` (either kind), or a TypeError.
fn this_stack(vm: &mut Vm, v: &Value, what: &str) -> JsResult<(Obj, bool, bool)> {
    if let Value::Object(o) = v {
        if let Kind::Disposable(d) = &vm.heap.get(*o).kind {
            return Ok((*o, d.is_async, d.disposed));
        }
    }
    vm.throw_type(&alloc::format!("{what} called on incompatible receiver"))
}

fn push_res(vm: &mut Vm, o: Obj, r: (Value, Value, u8)) {
    if let Kind::Disposable(d) = &mut vm.heap.get_mut(o).kind {
        d.res.push(r);
    }
}

fn not_disposed(vm: &mut Vm, disposed: bool) -> JsResult<()> {
    if disposed {
        return vm.throw_ref("The stack has already been disposed");
    }
    Ok(())
}

fn ds_use(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, is_async, disposed) = this_stack(vm, &ctx.this, "DisposableStack.prototype.use")?;
    not_disposed(vm, disposed)?;
    let v = vm.arg(ctx, 0);
    if v.is_nullish() {
        return Ok(v);
    }
    if !v.is_object() {
        return vm.throw_type("DisposableStack.prototype.use: the value is not an object");
    }
    let mut hint = 0u8;
    let mut m = None;
    if is_async {
        let ak = PropertyKey::Sym(vm.wk.async_dispose.clone());
        m = vm.get_method(&v, &ak)?;
    }
    if m.is_none() {
        let dk = PropertyKey::Sym(vm.wk.dispose.clone());
        m = vm.get_method(&v, &dk)?;
        if is_async {
            hint = 3;
        }
    }
    let Some(m) = m else { return vm.throw_type("The value has no [Symbol.dispose] method") };
    push_res(vm, o, (v.clone(), m, hint));
    Ok(v)
}

fn ds_adopt(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, _, disposed) = this_stack(vm, &ctx.this, "DisposableStack.prototype.adopt")?;
    not_disposed(vm, disposed)?;
    let v = vm.arg(ctx, 0);
    let f = vm.arg(ctx, 1);
    if !vm.is_callable(&f) {
        return vm.throw_type("onDispose is not a function");
    }
    push_res(vm, o, (v.clone(), f, 1));
    Ok(v)
}

fn ds_defer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, _, disposed) = this_stack(vm, &ctx.this, "DisposableStack.prototype.defer")?;
    not_disposed(vm, disposed)?;
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type("onDispose is not a function");
    }
    push_res(vm, o, (Value::Undefined, f, 2));
    Ok(Value::Undefined)
}

fn ds_move(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, is_async, disposed) = this_stack(vm, &ctx.this, "DisposableStack.prototype.move")?;
    not_disposed(vm, disposed)?;
    let res = match &mut vm.heap.get_mut(o).kind {
        Kind::Disposable(d) => {
            d.disposed = true;
            core::mem::take(&mut d.res)
        }
        _ => Vec::new(),
    };
    let p = if is_async { vm.intr().async_disposable_stack_proto } else { vm.intr().disposable_stack_proto };
    Ok(Value::Object(new_stack(vm, p, is_async, res)))
}

fn ds_disposed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, _, disposed) = this_stack(vm, &ctx.this, "get DisposableStack.prototype.disposed")?;
    Ok(Value::Bool(disposed))
}

/// Mark disposed and take the resources (in registration order).
fn take_for_dispose(vm: &mut Vm, o: Obj) -> Option<Vec<(Value, Value, u8)>> {
    match &mut vm.heap.get_mut(o).kind {
        Kind::Disposable(d) if !d.disposed => {
            d.disposed = true;
            Some(core::mem::take(&mut d.res))
        }
        _ => None,
    }
}

fn call_res(vm: &mut Vm, r: &(Value, Value, u8)) -> JsResult<Value> {
    match r.2 {
        0 | 3 => vm.call(&r.1, &r.0, &[]),
        1 => vm.call(&r.1, &Value::Undefined, &[r.0.clone()]),
        _ => vm.call(&r.1, &Value::Undefined, &[]),
    }
}

/// DisposableStack.prototype.dispose: DisposeResources in reverse order; errors chain as SuppressedError.
fn ds_dispose(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, is_async, _) = this_stack(vm, &ctx.this, "DisposableStack.prototype.dispose")?;
    if is_async {
        return vm.throw_type("DisposableStack.prototype.dispose called on an AsyncDisposableStack");
    }
    let Some(res) = take_for_dispose(vm, o) else { return Ok(Value::Undefined) };
    for r in &res {
        vm.root(&r.0);
        vm.root(&r.1);
    }
    let mut err: Option<Value> = None;
    for r in res.iter().rev() {
        if let Err(e) = call_res(vm, r) {
            vm.root(&e);
            err = Some(match err {
                Some(prev) => suppressed(vm, e, prev),
                None => e,
            });
        }
    }
    match err {
        Some(e) => Err(e),
        None => Ok(Value::Undefined),
    }
}

// AsyncDisposableStack.prototype.disposeAsync: a state record [resolve, reject, err, has_err, idx, (v, m, h)*].
const A_RES: usize = 0;
const A_REJ: usize = 1;
const A_ERR: usize = 2;
const A_HAS: usize = 3;
const A_IDX: usize = 4;
const A_BASE: usize = 5;

fn ads_dispose_async(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let pc = Value::Object(vm.intr().promise_ctor);
    let (p, res, rej) = new_capability(vm, &pc)?;
    let o = match &ctx.this {
        Value::Object(o) if matches!(&vm.heap.get(*o).kind, Kind::Disposable(d) if d.is_async) => *o,
        _ => {
            let e = vm.type_error("AsyncDisposableStack.prototype.disposeAsync called on incompatible receiver");
            vm.call(&rej, &Value::Undefined, &[e])?;
            return Ok(Value::Object(p));
        }
    };
    let Some(list) = take_for_dispose(vm, o) else {
        vm.call(&res, &Value::Undefined, &[Value::Undefined])?;
        return Ok(Value::Object(p));
    };
    let mut slots = vec![res, rej, Value::Undefined, Value::Bool(false), Value::Number(list.len() as f64)];
    for (v, m, h) in list {
        slots.push(v);
        slots.push(m);
        slots.push(Value::Number(h as f64));
    }
    let st = vm.alloc(ObjectData::new(None, Kind::Internal(slots)));
    ad_step(vm, st);
    Ok(Value::Object(p))
}

fn ad_record_error(vm: &mut Vm, st: Obj, e: Value) {
    let had = matches!(st_get(vm, st, A_HAS), Value::Bool(true));
    let v = if had {
        let prev = st_get(vm, st, A_ERR);
        suppressed(vm, e, prev)
    } else {
        e
    };
    st_set(vm, st, A_ERR, v);
    st_set(vm, st, A_HAS, Value::Bool(true));
}

fn ad_step(vm: &mut Vm, st: Obj) {
    loop {
        let idx = match st_get(vm, st, A_IDX) {
            Value::Number(n) => n as usize,
            _ => 0,
        };
        if idx == 0 {
            let (f, arg) = if matches!(st_get(vm, st, A_HAS), Value::Bool(true)) {
                (st_get(vm, st, A_REJ), st_get(vm, st, A_ERR))
            } else {
                (st_get(vm, st, A_RES), Value::Undefined)
            };
            let _ = vm.call(&f, &Value::Undefined, &[arg]);
            return;
        }
        let i = idx - 1;
        st_set(vm, st, A_IDX, Value::Number(i as f64));
        let b = A_BASE + 3 * i;
        let h = match st_get(vm, st, b + 2) {
            Value::Number(n) => n as u8,
            _ => 0,
        };
        let r = (st_get(vm, st, b), st_get(vm, st, b + 1), h);
        match call_res(vm, &r) {
            Ok(v) => {
                let v = if h == 3 { Value::Undefined } else { v };
                match promise_resolve_intrinsic(vm, v) {
                    Ok(pr) => {
                        let fo = vm.make_native_closure("", 1, ad_ok, vec![Value::Object(st)]);
                        let fe = vm.make_native_closure("", 1, ad_err, vec![Value::Object(st)]);
                        perform_then(vm, pr, Value::Object(fo), Value::Object(fe), None);
                        return;
                    }
                    Err(e) => ad_record_error(vm, st, e),
                }
            }
            Err(e) => ad_record_error(vm, st, e),
        }
    }
}

fn ad_ok(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    ad_step(vm, st);
    Ok(Value::Undefined)
}

fn ad_err(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let st = vm.native_slot(ctx.callee, 0).as_object().unwrap();
    let e = vm.arg(ctx, 0);
    ad_record_error(vm, st, e);
    ad_step(vm, st);
    Ok(Value::Undefined)
}

/// %IteratorPrototype%[@@dispose]: `this.return?.()`.
fn iterator_dispose(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let this = ctx.this.clone();
    if let Some(m) = vm.get_method(&this, &PropertyKey::from_str("return"))? {
        vm.call(&m, &this, &[])?;
    }
    Ok(Value::Undefined)
}

fn resolve_undefined(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let res = vm.native_slot(ctx.callee, 0);
    vm.call(&res, &Value::Undefined, &[Value::Undefined])
}

/// %AsyncIteratorPrototype%[@@asyncDispose]: await `this.return?.()`, fulfil with undefined.
fn async_iterator_async_dispose(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let pc = Value::Object(vm.intr().promise_ctor);
    let (p, res, rej) = new_capability(vm, &pc)?;
    let this = ctx.this.clone();
    let r = (|| -> JsResult<Option<Value>> {
        match vm.get_method(&this, &PropertyKey::from_str("return"))? {
            Some(m) => Ok(Some(vm.call(&m, &this, &[Value::Undefined])?)),
            None => Ok(None),
        }
    })();
    match r {
        Ok(None) => {
            vm.call(&res, &Value::Undefined, &[Value::Undefined])?;
        }
        Ok(Some(v)) => match promise_resolve_intrinsic(vm, v) {
            Ok(pr) => {
                let ok = vm.make_native_closure("", 1, resolve_undefined, vec![res]);
                perform_then(vm, pr, Value::Object(ok), rej, None);
            }
            Err(e) => {
                vm.call(&rej, &Value::Undefined, &[e])?;
            }
        },
        Err(e) => {
            vm.call(&rej, &Value::Undefined, &[e])?;
        }
    }
    Ok(Value::Object(p))
}

// ============================================================================================ `using` declarations

// A scope's DisposeCapability (ES2026 §9.1.1.x, §27.x DisposeResources) as an internal record:
// [has_error, error, needs_await, has_awaited, (value, method, hint)*], resources in registration order.
// Hints: 0 sync-dispose, 1 async-dispose through @@asyncDispose, 2 async-dispose through the @@dispose fallback
// (GetDisposeMethod's closure: the sync method is called, its result dropped, the closure's promise awaited).
const U_HAS: usize = 0;
const U_ERR: usize = 1;
const U_NEED: usize = 2;
const U_AWAITED: usize = 3;
const U_BASE: usize = 4;

/// NewDisposeCapability().
pub fn using_new(vm: &mut Vm) -> Value {
    let slots = vec![Value::Bool(false), Value::Undefined, Value::Bool(false), Value::Bool(false)];
    Value::Object(vm.alloc(ObjectData::new(None, Kind::Internal(slots))))
}

fn using_cap(v: &Value) -> Option<Obj> {
    v.as_object()
}

fn using_flag(vm: &Vm, cap: Obj, i: usize) -> bool {
    matches!(st_get(vm, cap, i), Value::Bool(true))
}

fn using_push(vm: &mut Vm, cap: Obj, v: Value, m: Value, hint: u8) {
    if let Kind::Internal(slots) = &mut vm.heap.get_mut(cap).kind {
        slots.push(v);
        slots.push(m);
        slots.push(Value::Number(hint as f64));
    }
}

/// AddDisposableResource(cap, V, hint) for a `using` (sync) or `await using` (async) binding: a nullish value is
/// skipped by `using` and recorded without a method by `await using` (it still implies an Await); any other
/// non-object is a TypeError; the method is GetDisposeMethod(V, hint) and must exist.
pub fn using_add(vm: &mut Vm, cap: &Value, v: &Value, is_async: bool) -> JsResult<()> {
    let Some(cap) = using_cap(cap) else { return Ok(()) };
    if v.is_nullish() {
        if is_async {
            using_push(vm, cap, Value::Undefined, Value::Undefined, 1);
        }
        return Ok(());
    }
    if !v.is_object() {
        let what = if is_async { "await using" } else { "using" };
        return vm.throw_type(&alloc::format!("{what}: the initializer is not an object"));
    }
    let mut hint = 0u8;
    let mut m = None;
    if is_async {
        let ak = PropertyKey::Sym(vm.wk.async_dispose.clone());
        m = vm.get_method(v, &ak)?;
        hint = 1;
    }
    if m.is_none() {
        let dk = PropertyKey::Sym(vm.wk.dispose.clone());
        m = vm.get_method(v, &dk)?;
        if is_async {
            hint = 2;
        }
    }
    let Some(m) = m else {
        let msg = if is_async { "await using: the value has no [Symbol.asyncDispose] or [Symbol.dispose] method" } else { "using: the value has no [Symbol.dispose] method" };
        return vm.throw_type(msg);
    };
    using_push(vm, cap, v.clone(), m, hint);
    Ok(())
}

/// A throw completion entering DisposeResources becomes the pending error.
pub fn using_seed(vm: &mut Vm, cap: &Value, kind: &Value, value: Value) {
    if let (Some(cap), Value::Number(k)) = (using_cap(cap), kind) {
        if *k == 1.0 {
            st_set(vm, cap, U_ERR, value);
            st_set(vm, cap, U_HAS, Value::Bool(true));
        }
    }
}

/// A disposal threw `e`: the completion becomes SuppressedError(e, previous) when an error is already pending.
pub fn using_error(vm: &mut Vm, cap: &Value, e: Value) {
    let Some(cap) = using_cap(cap) else { return };
    let v = if using_flag(vm, cap, U_HAS) {
        let prev = st_get(vm, cap, U_ERR);
        suppressed(vm, e, prev)
    } else {
        e
    };
    st_set(vm, cap, U_ERR, v);
    st_set(vm, cap, U_HAS, Value::Bool(true));
}

/// DisposeResources, resumable around its Awaits: dispose resources in reverse registration order and return
/// `Some(value)` when the caller must Await it (an async-dispose result, or the `Await(undefined)` owed by a
/// method-less `await using` resource before a sync one or at the end), `None` when the stack is empty.
pub fn using_step(vm: &mut Vm, cap: &Value) -> Option<Value> {
    let cap = using_cap(cap)?;
    loop {
        let len = match &vm.heap.get(cap).kind {
            Kind::Internal(slots) => slots.len(),
            _ => return None,
        };
        let need = using_flag(vm, cap, U_NEED);
        let awaited = using_flag(vm, cap, U_AWAITED);
        if len < U_BASE + 3 {
            if need && !awaited {
                st_set(vm, cap, U_NEED, Value::Bool(false));
                return Some(Value::Undefined);
            }
            return None;
        }
        let hint = match st_get(vm, cap, len - 1) {
            Value::Number(n) => n as u8,
            _ => 0,
        };
        if hint == 0 && need && !awaited {
            st_set(vm, cap, U_NEED, Value::Bool(false));
            return Some(Value::Undefined);
        }
        let (v, m) = match &mut vm.heap.get_mut(cap).kind {
            Kind::Internal(slots) => {
                slots.pop();
                let m = slots.pop().unwrap_or(Value::Undefined);
                let v = slots.pop().unwrap_or(Value::Undefined);
                (v, m)
            }
            _ => return None,
        };
        if m.is_undefined() {
            st_set(vm, cap, U_NEED, Value::Bool(true));
            continue;
        }
        let mark = vm.temp_roots.len();
        vm.temp_roots.push(v.clone());
        vm.temp_roots.push(m.clone());
        let r = vm.call(&m, &v, &[]);
        vm.temp_roots.truncate(mark);
        match (r, hint) {
            (Ok(_), 0) => {}
            (Ok(x), 1) => {
                st_set(vm, cap, U_AWAITED, Value::Bool(true));
                return Some(x);
            }
            (Ok(_), _) => {
                st_set(vm, cap, U_AWAITED, Value::Bool(true));
                return Some(Value::Undefined);
            }
            (Err(e), 2) => {
                // The fallback closure turns the throw into a rejected promise that is then awaited: the error
                // joins the completion now and the Await still happens.
                using_error(vm, &Value::Object(cap), e);
                st_set(vm, cap, U_AWAITED, Value::Bool(true));
                return Some(Value::Undefined);
            }
            (Err(e), _) => using_error(vm, &Value::Object(cap), e),
        }
    }
}

/// The end of DisposeResources: `Err(error)` when a disposal (or the completion) left one.
pub fn using_finish(vm: &mut Vm, cap: &Value) -> JsResult<()> {
    let Some(cap) = using_cap(cap) else { return Ok(()) };
    if using_flag(vm, cap, U_HAS) {
        let e = st_get(vm, cap, U_ERR);
        st_set(vm, cap, U_HAS, Value::Bool(false));
        st_set(vm, cap, U_ERR, Value::Undefined);
        return Err(e);
    }
    Ok(())
}
