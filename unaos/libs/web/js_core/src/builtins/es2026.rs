//! The ECMAScript 2026 additions a current page reaches for, beside the ES2025 library: Array.fromAsync
//! (§23.1.2.2), Error.isError (§20.5.2.1), Math.sumPrecise (§21.3.2.34), the Uint8Array base64 / hex
//! methods (§23.3.x: fromBase64, fromHex, toBase64, toHex, setFromBase64, setFromHex), Map / WeakMap
//! getOrInsert / getOrInsertComputed (the upsert proposal, stage 4), and Error.captureStackTrace (the
//! de-facto API every engine ships, which libraries call when present).

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
