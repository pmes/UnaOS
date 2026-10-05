//! ArrayBuffer and SharedArrayBuffer (§25.1, §25.2), DataView (§25.3), Atomics (§25.4), %TypedArray% and the
//! twelve concrete TypedArray constructors (§23.2), including resizable buffers and length-tracking views, and
//! the TypedArray exotic object's element access (§10.4.5).

use super::*;
use crate::bignum::BigUint;
use crate::vm::object::PropDesc;
use core::cmp::Ordering;

/// Largest buffer this engine allocates (a larger request is a RangeError, as CreateByteDataBlock permits).
pub const MAX_BUFFER: usize = 1 << 30;

pub const KINDS: [TAKind; 12] = [
    TAKind::Int8,
    TAKind::Uint8,
    TAKind::Uint8Clamped,
    TAKind::Int16,
    TAKind::Uint16,
    TAKind::Int32,
    TAKind::Uint32,
    TAKind::Float16,
    TAKind::Float32,
    TAKind::Float64,
    TAKind::BigInt64,
    TAKind::BigUint64,
];

fn kind_index(k: TAKind) -> usize {
    KINDS.iter().position(|x| *x == k).unwrap()
}

pub fn kind_name(k: TAKind) -> &'static str {
    match k {
        TAKind::Int8 => "Int8Array",
        TAKind::Uint8 => "Uint8Array",
        TAKind::Uint8Clamped => "Uint8ClampedArray",
        TAKind::Int16 => "Int16Array",
        TAKind::Uint16 => "Uint16Array",
        TAKind::Int32 => "Int32Array",
        TAKind::Uint32 => "Uint32Array",
        TAKind::Float16 => "Float16Array",
        TAKind::Float32 => "Float32Array",
        TAKind::Float64 => "Float64Array",
        TAKind::BigInt64 => "BigInt64Array",
        TAKind::BigUint64 => "BigUint64Array",
    }
}

// ================================================================================================ init

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let fp = vm.intr().function_proto;
    let r = vm.cur_realm as usize;

    // ArrayBuffer
    let abp = vm.new_object(Some(op));
    let abc = ctor(vm, "ArrayBuffer", 1, ab_ctor, abp);
    species_getter(vm, abc);
    method(vm, abc, "isView", 1, ab_is_view);
    for (n, f) in [("byteLength", ab_byte_length as NativeFn), ("maxByteLength", ab_max_byte_length), ("resizable", ab_resizable), ("detached", ab_detached)] {
        accessor(vm, abp, PropertyKey::from_str(n), n, Some(f), None, C);
    }
    for (n, l, f) in [("slice", 2, ab_slice as NativeFn), ("resize", 1, ab_resize), ("transfer", 0, ab_transfer), ("transferToFixedLength", 0, ab_transfer_fixed)] {
        method(vm, abp, n, l, f);
    }
    to_str_tag(vm, abp, "ArrayBuffer");
    vm.realms[r].intrinsics.array_buffer_proto = abp;
    vm.realms[r].intrinsics.array_buffer_ctor = abc;
    global(vm, "ArrayBuffer", Value::Object(abc));

    // SharedArrayBuffer
    let sbp = vm.new_object(Some(op));
    let sbc = ctor(vm, "SharedArrayBuffer", 1, sab_ctor, sbp);
    species_getter(vm, sbc);
    for (n, f) in [("byteLength", sab_byte_length as NativeFn), ("maxByteLength", sab_max_byte_length), ("growable", sab_growable)] {
        accessor(vm, sbp, PropertyKey::from_str(n), n, Some(f), None, C);
    }
    method(vm, sbp, "slice", 2, sab_slice);
    method(vm, sbp, "grow", 1, sab_grow);
    to_str_tag(vm, sbp, "SharedArrayBuffer");
    vm.realms[r].intrinsics.shared_array_buffer_proto = sbp;
    vm.realms[r].intrinsics.shared_array_buffer_ctor = sbc;
    global(vm, "SharedArrayBuffer", Value::Object(sbc));

    // DataView
    let dvp = vm.new_object(Some(op));
    let dvc = ctor(vm, "DataView", 1, dv_ctor, dvp);
    for (n, f) in [("buffer", dv_buffer as NativeFn), ("byteLength", dv_byte_length), ("byteOffset", dv_byte_offset)] {
        accessor(vm, dvp, PropertyKey::from_str(n), n, Some(f), None, C);
    }
    let dv_methods: [(&str, &str, TAKind); 11] = [
        ("getInt8", "setInt8", TAKind::Int8),
        ("getUint8", "setUint8", TAKind::Uint8),
        ("getInt16", "setInt16", TAKind::Int16),
        ("getUint16", "setUint16", TAKind::Uint16),
        ("getInt32", "setInt32", TAKind::Int32),
        ("getUint32", "setUint32", TAKind::Uint32),
        ("getFloat16", "setFloat16", TAKind::Float16),
        ("getFloat32", "setFloat32", TAKind::Float32),
        ("getFloat64", "setFloat64", TAKind::Float64),
        ("getBigInt64", "setBigInt64", TAKind::BigInt64),
        ("getBigUint64", "setBigUint64", TAKind::BigUint64),
    ];
    for (g, s, k) in dv_methods {
        let slot = alloc::vec![Value::Number(kind_index(k) as f64)];
        let gf = vm.make_native_with(g, 1, dv_get, false, Some(fp), slot.clone());
        vm.heap.get_mut(dvp).props.insert(PropertyKey::from_str(g), Prop::data(Value::Object(gf), WC));
        let sf = vm.make_native_with(s, 2, dv_set, false, Some(fp), slot);
        vm.heap.get_mut(dvp).props.insert(PropertyKey::from_str(s), Prop::data(Value::Object(sf), WC));
    }
    to_str_tag(vm, dvp, "DataView");
    vm.realms[r].intrinsics.data_view_proto = dvp;
    vm.realms[r].intrinsics.data_view_ctor = dvc;
    global(vm, "DataView", Value::Object(dvc));

    // %TypedArray%
    let tap = vm.new_object(Some(op));
    let tac = ctor(vm, "TypedArray", 0, ta_abstract_ctor, tap);
    species_getter(vm, tac);
    method(vm, tac, "from", 1, ta_from);
    method(vm, tac, "of", 0, ta_of);
    for (n, f) in [("buffer", ta_buffer as NativeFn), ("byteLength", ta_byte_length), ("byteOffset", ta_byte_offset), ("length", ta_length_getter)] {
        accessor(vm, tap, PropertyKey::from_str(n), n, Some(f), None, C);
    }
    let tag = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    accessor(vm, tap, tag, "[Symbol.toStringTag]", Some(ta_to_string_tag), None, C);
    for (n, l, f) in [
        ("at", 1, ta_at as NativeFn),
        ("copyWithin", 2, ta_copy_within),
        ("entries", 0, ta_entries),
        ("every", 1, ta_every),
        ("fill", 1, ta_fill),
        ("filter", 1, ta_filter),
        ("find", 1, ta_find),
        ("findIndex", 1, ta_find_index),
        ("findLast", 1, ta_find_last),
        ("findLastIndex", 1, ta_find_last_index),
        ("forEach", 1, ta_for_each),
        ("includes", 1, ta_includes),
        ("indexOf", 1, ta_index_of),
        ("join", 1, ta_join),
        ("keys", 0, ta_keys),
        ("lastIndexOf", 1, ta_last_index_of),
        ("map", 1, ta_map),
        ("reduce", 1, ta_reduce),
        ("reduceRight", 1, ta_reduce_right),
        ("reverse", 0, ta_reverse),
        ("set", 1, ta_set),
        ("slice", 2, ta_slice),
        ("some", 1, ta_some),
        ("sort", 1, ta_sort),
        ("subarray", 2, ta_subarray),
        ("toLocaleString", 0, ta_to_locale_string),
        ("toReversed", 0, ta_to_reversed),
        ("toSorted", 1, ta_to_sorted),
        ("with", 2, ta_with),
    ] {
        method(vm, tap, n, l, f);
    }
    let values = method(vm, tap, "values", 0, ta_values);
    let itk = PropertyKey::Sym(vm.wk.iterator.clone());
    vm.heap.get_mut(tap).props.insert(itk, Prop::data(Value::Object(values), WC));
    let ap = vm.intr().array_proto;
    if let Some(p) = vm.ordinary_get_own(ap, &PropertyKey::from_str("toString")) {
        vm.heap.get_mut(tap).props.insert(PropertyKey::from_str("toString"), p);
    }
    vm.realms[r].intrinsics.typed_array_proto = tap;
    vm.realms[r].intrinsics.typed_array_ctor = tac;

    let mut protos = Vec::new();
    let mut ctors = Vec::new();
    for (i, k) in KINDS.iter().enumerate() {
        let name = kind_name(*k);
        let p = vm.new_object(Some(tap));
        let c = vm.make_native_with(name, 3, ta_ctor, true, Some(tac), alloc::vec![Value::Number(i as f64)]);
        vm.heap.get_mut(c).props.insert(PropertyKey::from_str("prototype"), Prop::data(Value::Object(p), 0));
        vm.heap.get_mut(p).props.insert(PropertyKey::from_str("constructor"), Prop::data(Value::Object(c), WC));
        let bpe = Value::Number(k.size() as f64);
        value(vm, c, "BYTES_PER_ELEMENT", bpe.clone(), 0);
        value(vm, p, "BYTES_PER_ELEMENT", bpe, 0);
        global(vm, name, Value::Object(c));
        protos.push(p);
        ctors.push(c);
    }
    vm.realms[r].intrinsics.ta_protos = protos;
    vm.realms[r].intrinsics.ta_ctors = ctors;

    // Atomics
    let at = vm.new_object(Some(op));
    for (n, l, f) in [
        ("add", 3, atomics_add as NativeFn),
        ("and", 3, atomics_and),
        ("compareExchange", 4, atomics_compare_exchange),
        ("exchange", 3, atomics_exchange),
        ("isLockFree", 1, atomics_is_lock_free),
        ("load", 2, atomics_load),
        ("notify", 3, atomics_notify),
        ("or", 3, atomics_or),
        ("store", 3, atomics_store),
        ("sub", 3, atomics_sub),
        ("wait", 4, atomics_wait),
        ("xor", 3, atomics_xor),
    ] {
        method(vm, at, n, l, f);
    }
    to_str_tag(vm, at, "Atomics");
    vm.realms[r].intrinsics.atomics = at;
    global(vm, "Atomics", Value::Object(at));
}

// ================================================================================================ buffers

fn buffer_data(vm: &Vm, o: Obj) -> Option<&BufferData> {
    match &vm.heap.get(o).kind {
        Kind::ArrayBuffer(b) => Some(b),
        _ => None,
    }
}

fn buffer_mut(vm: &mut Vm, o: Obj) -> &mut BufferData {
    match &mut vm.heap.get_mut(o).kind {
        Kind::ArrayBuffer(b) => b,
        _ => unreachable!(),
    }
}

fn is_detached(vm: &Vm, b: Obj) -> bool {
    buffer_data(vm, b).map(|d| d.detached).unwrap_or(true)
}

fn buf_len(vm: &Vm, b: Obj) -> usize {
    buffer_data(vm, b).map(|d| d.data.len()).unwrap_or(0)
}

fn is_fixed_buffer(vm: &Vm, b: Obj) -> bool {
    buffer_data(vm, b).map(|d| d.max_len.is_none()).unwrap_or(true)
}

fn new_block(vm: &mut Vm, len: usize) -> JsResult<Vec<u8>> {
    if len > MAX_BUFFER {
        return vm.throw_range("Array buffer allocation failed");
    }
    let mut v = Vec::new();
    if v.try_reserve_exact(len).is_err() {
        return vm.throw_range("Array buffer allocation failed");
    }
    v.resize(len, 0);
    Ok(v)
}

/// AllocateArrayBuffer / AllocateSharedArrayBuffer with a resolved prototype.
fn alloc_buffer(vm: &mut Vm, proto: Obj, len: usize, max: Option<usize>, shared: bool) -> JsResult<Obj> {
    if let Some(m) = max {
        if len > m {
            return vm.throw_range("byteLength exceeds maxByteLength");
        }
        if m > MAX_BUFFER {
            return vm.throw_range("Array buffer allocation failed");
        }
    }
    let data = new_block(vm, len)?;
    Ok(vm.alloc(ObjectData::new(Some(proto), Kind::ArrayBuffer(Box::new(BufferData { data, detached: false, shared, max_len: max })))))
}

pub fn new_array_buffer(vm: &mut Vm, len: usize) -> JsResult<Obj> {
    let p = vm.intr().array_buffer_proto;
    alloc_buffer(vm, p, len, None, false)
}

fn max_len_option(vm: &mut Vm, options: &Value) -> JsResult<Option<usize>> {
    if !options.is_object() {
        return Ok(None);
    }
    let m = vm.get_v(options, &PropertyKey::from_str("maxByteLength"))?;
    if m.is_undefined() {
        return Ok(None);
    }
    Ok(Some(vm.to_index(&m)?))
}

fn ab_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor ArrayBuffer requires 'new'");
    }
    let l = vm.arg(ctx, 0);
    let len = vm.to_index(&l)?;
    let opts = vm.arg(ctx, 1);
    let max = max_len_option(vm, &opts)?;
    if let Some(m) = max {
        if len > m {
            return vm.throw_range("byteLength exceeds maxByteLength");
        }
    }
    let nt = ctx.new_target.clone();
    let proto = vm.get_prototype_from_ctor(&nt, |i| i.array_buffer_proto)?;
    Ok(Value::Object(alloc_buffer(vm, proto, len, max, false)?))
}

fn sab_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor SharedArrayBuffer requires 'new'");
    }
    let l = vm.arg(ctx, 0);
    let len = vm.to_index(&l)?;
    let opts = vm.arg(ctx, 1);
    let max = max_len_option(vm, &opts)?;
    if let Some(m) = max {
        if len > m {
            return vm.throw_range("byteLength exceeds maxByteLength");
        }
    }
    let nt = ctx.new_target.clone();
    let proto = vm.get_prototype_from_ctor(&nt, |i| i.shared_array_buffer_proto)?;
    Ok(Value::Object(alloc_buffer(vm, proto, len, max, true)?))
}

fn ab_is_view(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    Ok(Value::Bool(match a {
        Value::Object(o) => matches!(vm.heap.get(o).kind, Kind::TypedArray(_) | Kind::DataView(_)),
        _ => false,
    }))
}

fn this_buffer(vm: &mut Vm, ctx: &CallCtx, shared: bool, name: &str) -> JsResult<Obj> {
    if let Value::Object(o) = &ctx.this {
        if let Some(d) = buffer_data(vm, *o) {
            if d.shared == shared {
                return Ok(*o);
            }
        }
    }
    let what = if shared { "SharedArrayBuffer" } else { "ArrayBuffer" };
    vm.throw_type(&alloc::format!("{}.prototype.{} called on incompatible receiver", what, name))
}

fn ab_byte_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_buffer(vm, ctx, false, "byteLength")?;
    Ok(Value::Number(buf_len(vm, b) as f64))
}
fn ab_max_byte_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_buffer(vm, ctx, false, "maxByteLength")?;
    let d = buffer_data(vm, b).unwrap();
    Ok(Value::Number(if d.detached { 0 } else { d.max_len.unwrap_or(d.data.len()) } as f64))
}
fn ab_resizable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_buffer(vm, ctx, false, "resizable")?;
    Ok(Value::Bool(!is_fixed_buffer(vm, b)))
}
fn ab_detached(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_buffer(vm, ctx, false, "detached")?;
    Ok(Value::Bool(is_detached(vm, b)))
}
fn sab_byte_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_buffer(vm, ctx, true, "byteLength")?;
    Ok(Value::Number(buf_len(vm, b) as f64))
}
fn sab_max_byte_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_buffer(vm, ctx, true, "maxByteLength")?;
    let d = buffer_data(vm, b).unwrap();
    Ok(Value::Number(d.max_len.unwrap_or(d.data.len()) as f64))
}
fn sab_growable(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let b = this_buffer(vm, ctx, true, "growable")?;
    Ok(Value::Bool(!is_fixed_buffer(vm, b)))
}

fn rel(vm: &mut Vm, v: &Value, len: f64, default: f64) -> JsResult<usize> {
    Ok(relative_index(vm, v, len, default)? as usize)
}

fn buffer_slice(vm: &mut Vm, ctx: &CallCtx, shared: bool) -> JsResult<Value> {
    let o = this_buffer(vm, ctx, shared, "slice")?;
    if is_detached(vm, o) {
        return vm.throw_type("Cannot perform ArrayBuffer.prototype.slice on a detached ArrayBuffer");
    }
    let len = buf_len(vm, o) as f64;
    let s = vm.arg(ctx, 0);
    let e = vm.arg(ctx, 1);
    let first = rel(vm, &s, len, 0.0)?;
    let fin = rel(vm, &e, len, len)?;
    let new_len = fin.saturating_sub(first);
    let dc = if shared { vm.intr().shared_array_buffer_ctor } else { vm.intr().array_buffer_ctor };
    let c = vm.species_constructor(o, dc)?;
    let nb = vm.construct(&c, &[Value::Number(new_len as f64)], None)?;
    let n = match &nb {
        Value::Object(n) if buffer_data(vm, *n).map(|d| d.shared == shared).unwrap_or(false) => *n,
        _ => return vm.throw_type("Species constructor did not return an ArrayBuffer"),
    };
    if !shared && is_detached(vm, n) {
        return vm.throw_type("Species constructor returned a detached ArrayBuffer");
    }
    if n == o {
        return vm.throw_type("Species constructor returned the same ArrayBuffer");
    }
    if buf_len(vm, n) < new_len {
        return vm.throw_type("Species constructor returned a too small ArrayBuffer");
    }
    if is_detached(vm, o) {
        return vm.throw_type("ArrayBuffer was detached");
    }
    let cur = buf_len(vm, o);
    if first < cur {
        let count = new_len.min(cur - first);
        let bytes = buffer_data(vm, o).unwrap().data[first..first + count].to_vec();
        buffer_mut(vm, n).data[..count].copy_from_slice(&bytes);
    }
    Ok(nb)
}

fn ab_slice(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    buffer_slice(vm, ctx, false)
}
fn sab_slice(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    buffer_slice(vm, ctx, true)
}

fn ab_resize(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_buffer(vm, ctx, false, "resize")?;
    let max = match buffer_data(vm, o).unwrap().max_len {
        Some(m) => m,
        None => return vm.throw_type("ArrayBuffer.prototype.resize called on a fixed-length ArrayBuffer"),
    };
    let nl = vm.arg(ctx, 0);
    let n = vm.to_index(&nl)?;
    if is_detached(vm, o) {
        return vm.throw_type("Cannot resize a detached ArrayBuffer");
    }
    if n > max {
        return vm.throw_range("new length exceeds maxByteLength");
    }
    buffer_mut(vm, o).data.resize(n, 0);
    Ok(Value::Undefined)
}

fn sab_grow(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let o = this_buffer(vm, ctx, true, "grow")?;
    let max = match buffer_data(vm, o).unwrap().max_len {
        Some(m) => m,
        None => return vm.throw_type("SharedArrayBuffer.prototype.grow called on a fixed-length SharedArrayBuffer"),
    };
    let nl = vm.arg(ctx, 0);
    let n = vm.to_index(&nl)?;
    let cur = buf_len(vm, o);
    if n > max || n < cur {
        return vm.throw_range("invalid length for SharedArrayBuffer.prototype.grow");
    }
    buffer_mut(vm, o).data.resize(n, 0);
    Ok(Value::Undefined)
}

/// ArrayBufferCopyAndDetach (§25.1.3.3)
fn copy_and_detach(vm: &mut Vm, ctx: &CallCtx, preserve: bool) -> JsResult<Value> {
    let o = this_buffer(vm, ctx, false, if preserve { "transfer" } else { "transferToFixedLength" })?;
    let nl = vm.arg(ctx, 0);
    let new_len = if nl.is_undefined() { buf_len(vm, o) } else { vm.to_index(&nl)? };
    if is_detached(vm, o) {
        return vm.throw_type("Cannot transfer a detached ArrayBuffer");
    }
    let max = if preserve { buffer_data(vm, o).unwrap().max_len } else { None };
    let p = vm.intr().array_buffer_proto;
    let nb = alloc_buffer(vm, p, new_len, max, false)?;
    let src = core::mem::take(&mut buffer_mut(vm, o).data);
    let n = new_len.min(src.len());
    buffer_mut(vm, nb).data[..n].copy_from_slice(&src[..n]);
    buffer_mut(vm, o).detached = true;
    Ok(Value::Object(nb))
}
fn ab_transfer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    copy_and_detach(vm, ctx, true)
}
fn ab_transfer_fixed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    copy_and_detach(vm, ctx, false)
}

/// DetachArrayBuffer (host hook for $262.detachArrayBuffer).
pub fn detach_array_buffer(vm: &mut Vm, b: &Value, _key: &Value) -> JsResult<()> {
    match b {
        Value::Object(o) => match &mut vm.heap.get_mut(*o).kind {
            Kind::ArrayBuffer(d) => {
                if d.shared {
                    return vm.throw_type("Cannot detach a SharedArrayBuffer");
                }
                d.detached = true;
                d.data = Vec::new();
                Ok(())
            }
            _ => vm.throw_type("not an ArrayBuffer"),
        },
        _ => vm.throw_type("not an ArrayBuffer"),
    }
}

// ================================================================================================ raw element I/O

fn f16_bits(x: f64) -> u16 {
    let r = crate::builtins::math::f16round(x);
    let sign: u16 = if r.is_sign_negative() { 0x8000 } else { 0 };
    if r.is_nan() {
        return 0x7E00;
    }
    let a = r.abs();
    if a == f64::INFINITY {
        return sign | 0x7C00;
    }
    if a == 0.0 {
        return sign;
    }
    let (m, e) = crate::builtins::math::frexp(a); // a = m * 2^e, m in [0.5, 1)
    let exp = e - 1; // a = 1.f * 2^exp
    if exp < -14 {
        // subnormal: a = frac * 2^-24
        let frac = crate::builtins::math::ldexp(a, 24) as u16;
        return sign | frac;
    }
    let frac = ((m * 2.0 - 1.0) * 1024.0) as u16;
    sign | (((exp + 15) as u16) << 10) | frac
}

fn f16_value(b: u16) -> f64 {
    let sign = if b & 0x8000 != 0 { -1.0 } else { 1.0 };
    let e = ((b >> 10) & 0x1F) as i32;
    let f = (b & 0x3FF) as f64;
    let v = if e == 0 {
        crate::builtins::math::ldexp(f, -24)
    } else if e == 31 {
        if f == 0.0 { f64::INFINITY } else { f64::NAN }
    } else {
        crate::builtins::math::ldexp(1.0 + f / 1024.0, e - 15)
    };
    sign * v
}

fn bigint_to_u64(b: &BigInt) -> u64 {
    let lo = b.mag.low_u64();
    if b.neg { lo.wrapping_neg() } else { lo }
}

fn uint8_clamp(n: f64) -> u8 {
    if n.is_nan() || n <= 0.0 {
        return 0;
    }
    if n >= 255.0 {
        return 255;
    }
    let f = crate::numconv::libm_floor(n);
    if f + 0.5 < n {
        return (f + 1.0) as u8;
    }
    if n < f + 0.5 {
        return f as u8;
    }
    let fi = f as u8;
    if fi % 2 == 0 { fi } else { fi + 1 }
}

/// Encode a numeric value (Number for numeric kinds, BigInt for bigint kinds) as little-endian bytes.
fn encode(kind: TAKind, v: &Value) -> [u8; 8] {
    let mut out = [0u8; 8];
    match (kind, v) {
        (TAKind::BigInt64 | TAKind::BigUint64, Value::BigInt(b)) => out = bigint_to_u64(b).to_le_bytes(),
        (_, Value::Number(n)) => {
            let n = *n;
            match kind {
                TAKind::Int8 | TAKind::Uint8 => out[0] = crate::vm::ops::to_int32(n) as u8,
                TAKind::Uint8Clamped => out[0] = uint8_clamp(n),
                TAKind::Int16 | TAKind::Uint16 => out[..2].copy_from_slice(&(crate::vm::ops::to_int32(n) as u16).to_le_bytes()),
                TAKind::Int32 | TAKind::Uint32 => out[..4].copy_from_slice(&(crate::vm::ops::to_int32(n) as u32).to_le_bytes()),
                TAKind::Float16 => out[..2].copy_from_slice(&f16_bits(n).to_le_bytes()),
                TAKind::Float32 => out[..4].copy_from_slice(&(n as f32).to_bits().to_le_bytes()),
                TAKind::Float64 => out = n.to_bits().to_le_bytes(),
                _ => {}
            }
        }
        _ => {}
    }
    out
}

fn decode(kind: TAKind, b: &[u8]) -> Value {
    let u16le = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    let u32le = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
    let u64le = |b: &[u8]| u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]);
    match kind {
        TAKind::Int8 => Value::Number(b[0] as i8 as f64),
        TAKind::Uint8 | TAKind::Uint8Clamped => Value::Number(b[0] as f64),
        TAKind::Int16 => Value::Number(u16le(b) as i16 as f64),
        TAKind::Uint16 => Value::Number(u16le(b) as f64),
        TAKind::Int32 => Value::Number(u32le(b) as i32 as f64),
        TAKind::Uint32 => Value::Number(u32le(b) as f64),
        TAKind::Float16 => Value::Number(f16_value(u16le(b))),
        TAKind::Float32 => Value::Number(f32::from_bits(u32le(b)) as f64),
        TAKind::Float64 => Value::Number(f64::from_bits(u64le(b))),
        TAKind::BigInt64 => Value::BigInt(Rc::new(BigInt::from_i64(u64le(b) as i64))),
        TAKind::BigUint64 => Value::BigInt(Rc::new(BigInt::from_mag(false, BigUint::from_u64(u64le(b))))),
    }
}

/// GetValueFromBuffer
fn get_raw(vm: &Vm, buf: Obj, byte_index: usize, kind: TAKind, little: bool) -> Value {
    let d = buffer_data(vm, buf).unwrap();
    let n = kind.size();
    let mut b = [0u8; 8];
    b[..n].copy_from_slice(&d.data[byte_index..byte_index + n]);
    if !little {
        b[..n].reverse();
    }
    decode(kind, &b[..n])
}

/// SetValueInBuffer (value already converted to the kind's numeric type).
fn set_raw(vm: &mut Vm, buf: Obj, byte_index: usize, kind: TAKind, v: &Value, little: bool) {
    let n = kind.size();
    let mut b = encode(kind, v);
    if !little {
        b[..n].reverse();
    }
    buffer_mut(vm, buf).data[byte_index..byte_index + n].copy_from_slice(&b[..n]);
}

/// ToBigInt or ToNumber per the kind's content type.
fn to_kind_numeric(vm: &mut Vm, kind: TAKind, v: &Value) -> JsResult<Value> {
    if kind.is_bigint() {
        Ok(Value::BigInt(vm.to_bigint(v)?))
    } else {
        Ok(Value::Number(vm.to_number(v)?))
    }
}

// ================================================================================================ TypedArray records

#[derive(Clone, Copy)]
struct Ta {
    kind: TAKind,
    buffer: Obj,
    offset: usize,
    length: Option<usize>,
}

fn ta_rec(vm: &Vm, o: Obj) -> Option<Ta> {
    match &vm.heap.get(o).kind {
        Kind::TypedArray(t) => Some(Ta { kind: t.kind, buffer: t.buffer, offset: t.byte_offset, length: t.length }),
        _ => None,
    }
}

/// IsTypedArrayOutOfBounds (detached counts as out of bounds).
fn oob(vm: &Vm, t: &Ta) -> bool {
    if is_detached(vm, t.buffer) {
        return true;
    }
    let bl = buf_len(vm, t.buffer);
    let end = match t.length {
        None => bl,
        Some(n) => t.offset + n * t.kind.size(),
    };
    t.offset > bl || end > bl
}

/// TypedArrayLength (None when out of bounds).
fn len_of(vm: &Vm, t: &Ta) -> Option<usize> {
    if oob(vm, t) {
        return None;
    }
    Some(match t.length {
        Some(n) => n,
        None => (buf_len(vm, t.buffer) - t.offset) / t.kind.size(),
    })
}

pub fn ta_length(vm: &Vm, o: Obj) -> Option<usize> {
    ta_rec(vm, o).and_then(|t| len_of(vm, &t))
}

pub fn is_fixed_length(vm: &Vm, o: Obj) -> bool {
    match ta_rec(vm, o) {
        Some(t) => t.length.is_some() && is_fixed_buffer(vm, t.buffer),
        None => true,
    }
}

/// CanonicalNumericIndexString for a property key.
pub fn canonical_index(key: &PropertyKey) -> Option<f64> {
    match key {
        PropertyKey::Index(i) => Some(*i as f64),
        PropertyKey::Str(s) => {
            if s.eq_str("-0") {
                return Some(-0.0);
            }
            let u = s.units();
            // Fast reject: canonical numeric strings start with a digit, '-', 'I' (Infinity) or 'N' (NaN).
            match u.first() {
                Some(c) if (*c >= b'0' as u16 && *c <= b'9' as u16) || *c == b'-' as u16 || *c == b'I' as u16 || *c == b'N' as u16 => {}
                _ => return None,
            }
            let n = crate::vm::ops::string_to_number(s);
            let back = crate::vm::ops::number_to_jsstr(n);
            if back.units() == u { Some(n) } else { None }
        }
        _ => None,
    }
}

/// IsValidIntegerIndex
fn valid_index(vm: &Vm, t: &Ta, i: f64) -> Option<usize> {
    if is_detached(vm, t.buffer) {
        return None;
    }
    if !i.is_finite() || crate::numconv::libm_floor(i) != i || (i == 0.0 && i.is_sign_negative()) || i < 0.0 {
        return None;
    }
    let len = len_of(vm, t)?;
    if i >= len as f64 {
        return None;
    }
    Some(i as usize)
}

pub fn ta_valid_index(vm: &Vm, o: Obj, i: f64) -> bool {
    ta_rec(vm, o).map(|t| valid_index(vm, &t, i).is_some()).unwrap_or(false)
}

/// TypedArrayGetElement
pub fn ta_get_index(vm: &Vm, o: Obj, i: f64) -> Option<Value> {
    let t = ta_rec(vm, o)?;
    let idx = valid_index(vm, &t, i)?;
    Some(get_raw(vm, t.buffer, t.offset + idx * t.kind.size(), t.kind, true))
}

/// TypedArraySetElement: convert first, then write if the index is (still) valid.
pub fn ta_set_index(vm: &mut Vm, o: Obj, i: f64, v: &Value) -> JsResult<()> {
    let t = ta_rec(vm, o).unwrap();
    let nv = to_kind_numeric(vm, t.kind, v)?;
    let t = ta_rec(vm, o).unwrap();
    if let Some(idx) = valid_index(vm, &t, i) {
        set_raw(vm, t.buffer, t.offset + idx * t.kind.size(), t.kind, &nv, true);
    }
    Ok(())
}

/// [[DefineOwnProperty]] for a numeric key (§10.4.5.3).
pub fn ta_define_index(vm: &mut Vm, o: Obj, i: f64, d: PropDesc) -> JsResult<bool> {
    if !ta_valid_index(vm, o, i) {
        return Ok(false);
    }
    if d.configurable == Some(false) || d.enumerable == Some(false) || d.is_accessor() || d.writable == Some(false) {
        return Ok(false);
    }
    if let Some(v) = &d.value {
        ta_set_index(vm, o, i, v)?;
    }
    Ok(true)
}

fn elem(vm: &Vm, t: &Ta, k: usize) -> Value {
    // Re-validated read (undefined once out of bounds).
    match valid_index(vm, t, k as f64) {
        Some(idx) => get_raw(vm, t.buffer, t.offset + idx * t.kind.size(), t.kind, true),
        None => Value::Undefined,
    }
}

/// ValidateTypedArray: the record and its current length.
fn validate(vm: &mut Vm, v: &Value) -> JsResult<(Obj, Ta, usize)> {
    if let Value::Object(o) = v {
        if let Some(t) = ta_rec(vm, *o) {
            return match len_of(vm, &t) {
                Some(n) => Ok((*o, t, n)),
                None => vm.throw_type("TypedArray is detached or out of bounds"),
            };
        }
    }
    vm.throw_type("this is not a typed array")
}

fn require_ta(vm: &mut Vm, v: &Value) -> JsResult<(Obj, Ta)> {
    if let Value::Object(o) = v {
        if let Some(t) = ta_rec(vm, *o) {
            return Ok((*o, t));
        }
    }
    vm.throw_type("this is not a typed array")
}

fn alloc_ta(vm: &mut Vm, proto: Obj, kind: TAKind, buffer: Obj, offset: usize, length: Option<usize>) -> Obj {
    vm.alloc(ObjectData::new(Some(proto), Kind::TypedArray(Box::new(TypedArrayData { kind, buffer, byte_offset: offset, length }))))
}

/// A new typed array of `kind` with a fresh zeroed buffer.
fn create_with_length(vm: &mut Vm, proto: Obj, kind: TAKind, len: usize) -> JsResult<Obj> {
    let bytes = match len.checked_mul(kind.size()) {
        Some(b) => b,
        None => return vm.throw_range("Invalid typed array length"),
    };
    let buf = new_array_buffer(vm, bytes)?;
    Ok(alloc_ta(vm, proto, kind, buf, 0, Some(len)))
}

// ================================================================================================ constructors

fn ta_abstract_ctor(vm: &mut Vm, _ctx: &CallCtx) -> JsResult<Value> {
    vm.throw_type("Abstract class TypedArray not directly constructable")
}

fn ta_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ki = match vm.native_slot(ctx.callee, 0) {
        Value::Number(n) => n as usize,
        _ => 0,
    };
    let kind = KINDS[ki];
    if ctx.new_target.is_undefined() {
        return vm.throw_type(&alloc::format!("Constructor {} requires 'new'", kind_name(kind)));
    }
    let nt = ctx.new_target.clone();
    let first = vm.arg(ctx, 0);
    let first_obj = match &first {
        Value::Object(o) => Some(*o),
        _ => None,
    };
    let Some(src) = first_obj else {
        let len = vm.to_index(&first)?;
        let proto = vm.get_prototype_from_ctor(&nt, move |i| i.ta_protos[ki])?;
        return Ok(Value::Object(create_with_length(vm, proto, kind, len)?));
    };
    let proto = vm.get_prototype_from_ctor(&nt, move |i| i.ta_protos[ki])?;
    if let Some(st) = ta_rec(vm, src) {
        // InitializeTypedArrayFromTypedArray
        let len = match len_of(vm, &st) {
            Some(n) => n,
            None => return vm.throw_type("source TypedArray is detached or out of bounds"),
        };
        if st.kind.is_bigint() != kind.is_bigint() {
            return vm.throw_type("Content types of source and target typed arrays differ");
        }
        let o = create_with_length(vm, proto, kind, len)?;
        let t = ta_rec(vm, o).unwrap();
        if st.kind == kind {
            let n = len * kind.size();
            let bytes = buffer_data(vm, st.buffer).unwrap().data[st.offset..st.offset + n].to_vec();
            buffer_mut(vm, t.buffer).data[..n].copy_from_slice(&bytes);
        } else {
            for k in 0..len {
                let v = get_raw(vm, st.buffer, st.offset + k * st.kind.size(), st.kind, true);
                set_raw(vm, t.buffer, k * kind.size(), kind, &v, true);
            }
        }
        return Ok(Value::Object(o));
    }
    if buffer_data(vm, src).is_some() {
        // InitializeTypedArrayFromArrayBuffer
        let es = kind.size();
        let bo = vm.arg(ctx, 1);
        let offset = vm.to_index(&bo)?;
        if offset % es != 0 {
            return vm.throw_range(&alloc::format!("start offset of {} should be a multiple of {}", kind_name(kind), es));
        }
        let fixed = is_fixed_buffer(vm, src);
        let lv = vm.arg(ctx, 2);
        let new_len = if lv.is_undefined() { None } else { Some(vm.to_index(&lv)?) };
        if is_detached(vm, src) {
            return vm.throw_type("Cannot construct a TypedArray on a detached ArrayBuffer");
        }
        let bl = buf_len(vm, src);
        let length = match new_len {
            None if !fixed => {
                if offset > bl {
                    return vm.throw_range("Start offset is outside the bounds of the buffer");
                }
                None
            }
            None => {
                if bl % es != 0 {
                    return vm.throw_range(&alloc::format!("byte length of {} should be a multiple of {}", kind_name(kind), es));
                }
                if offset > bl {
                    return vm.throw_range("Start offset is outside the bounds of the buffer");
                }
                Some((bl - offset) / es)
            }
            Some(n) => {
                if offset as u128 + n as u128 * es as u128 > bl as u128 {
                    return vm.throw_range("Invalid typed array length");
                }
                Some(n)
            }
        };
        return Ok(Value::Object(alloc_ta(vm, proto, kind, src, offset, length)));
    }
    // Iterable or array-like.
    let itk = PropertyKey::Sym(vm.wk.iterator.clone());
    let using = vm.get_method(&first, &itk)?;
    let values: Vec<Value> = match using {
        Some(m) => {
            let (it, next) = vm.get_iterator_from_method(&first, &m)?;
            let mut out = Vec::new();
            while let Some(v) = vm.iterator_step_value(&it, &next)? {
                vm.root(&v);
                out.push(v);
            }
            out
        }
        None => {
            let len = vm.length_of(src)?;
            let o = create_with_length(vm, proto, kind, len as usize)?;
            for k in 0..len as usize {
                let v = vm.get(src, &PropertyKey::from(k as u32))?;
                vm.set_prop(o, PropertyKey::from(k as u32), v, true)?;
            }
            return Ok(Value::Object(o));
        }
    };
    let o = create_with_length(vm, proto, kind, values.len())?;
    for (k, v) in values.into_iter().enumerate() {
        vm.set_prop(o, PropertyKey::from(k as u32), v, true)?;
    }
    Ok(Value::Object(o))
}

/// TypedArrayCreateFromConstructor
fn create_from_ctor(vm: &mut Vm, c: &Value, args: &[Value]) -> JsResult<Obj> {
    let nv = vm.construct(c, args, None)?;
    let (o, _t, len) = validate(vm, &nv)?;
    if args.len() == 1 {
        if let Value::Number(n) = &args[0] {
            if (len as f64) < *n {
                return vm.throw_type("Derived TypedArray constructor created an array which was too small");
            }
        }
    }
    vm.root(&nv);
    Ok(o)
}

/// TypedArraySpeciesCreate
fn species_create(vm: &mut Vm, exemplar: Obj, args: &[Value]) -> JsResult<Obj> {
    let t = ta_rec(vm, exemplar).unwrap();
    let dc = vm.intr().ta_ctors[kind_index(t.kind)];
    let c = vm.species_constructor(exemplar, dc)?;
    let r = create_from_ctor(vm, &c, args)?;
    let rt = ta_rec(vm, r).unwrap();
    if rt.kind.is_bigint() != t.kind.is_bigint() {
        return vm.throw_type("Content type of species-created TypedArray differs");
    }
    Ok(r)
}

/// TypedArrayCreateSameType
fn create_same_type(vm: &mut Vm, t: &Ta, len: usize) -> JsResult<Obj> {
    let c = Value::Object(vm.intr().ta_ctors[kind_index(t.kind)]);
    create_from_ctor(vm, &c, &[Value::Number(len as f64)])
}

fn ta_from(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = ctx.this.clone();
    if !vm.is_constructor(&c) {
        return vm.throw_type("TypedArray.from: this is not a constructor");
    }
    let source = vm.arg(ctx, 0);
    let mapfn = vm.arg(ctx, 1);
    let this_arg = vm.arg(ctx, 2);
    let mapping = !mapfn.is_undefined();
    if mapping && !vm.is_callable(&mapfn) {
        return vm.throw_type("TypedArray.from: mapper is not a function");
    }
    let itk = PropertyKey::Sym(vm.wk.iterator.clone());
    let using = vm.get_method(&source, &itk)?;
    if let Some(m) = using {
        let (it, next) = vm.get_iterator_from_method(&source, &m)?;
        let mut values = Vec::new();
        while let Some(v) = vm.iterator_step_value(&it, &next)? {
            vm.root(&v);
            values.push(v);
        }
        let target = create_from_ctor(vm, &c, &[Value::Number(values.len() as f64)])?;
        for (k, v) in values.into_iter().enumerate() {
            let mv = if mapping { vm.call(&mapfn, &this_arg, &[v, Value::Number(k as f64)])? } else { v };
            vm.set_prop(target, PropertyKey::from(k as u32), mv, true)?;
        }
        return Ok(Value::Object(target));
    }
    let al = vm.to_object(&source)?.as_object().unwrap();
    let len = vm.length_of(al)?;
    let target = create_from_ctor(vm, &c, &[Value::Number(len)])?;
    for k in 0..len as usize {
        let key = PropertyKey::from(k as u32);
        let v = vm.get(al, &key)?;
        let mv = if mapping { vm.call(&mapfn, &this_arg, &[v, Value::Number(k as f64)])? } else { v };
        vm.set_prop(target, key, mv, true)?;
    }
    Ok(Value::Object(target))
}

fn ta_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let c = ctx.this.clone();
    if !vm.is_constructor(&c) {
        return vm.throw_type("TypedArray.of: this is not a constructor");
    }
    let items = vm.args(ctx);
    let o = create_from_ctor(vm, &c, &[Value::Number(items.len() as f64)])?;
    for (k, v) in items.into_iter().enumerate() {
        vm.set_prop(o, PropertyKey::from(k as u32), v, true)?;
    }
    Ok(Value::Object(o))
}

// ================================================================================================ accessors

fn ta_buffer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t) = require_ta(vm, &ctx.this)?;
    Ok(Value::Object(t.buffer))
}
fn ta_byte_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t) = require_ta(vm, &ctx.this)?;
    Ok(Value::Number(len_of(vm, &t).map(|n| n * t.kind.size()).unwrap_or(0) as f64))
}
fn ta_byte_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t) = require_ta(vm, &ctx.this)?;
    Ok(Value::Number(if oob(vm, &t) { 0 } else { t.offset } as f64))
}
fn ta_length_getter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t) = require_ta(vm, &ctx.this)?;
    Ok(Value::Number(len_of(vm, &t).unwrap_or(0) as f64))
}
fn ta_to_string_tag(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if let Value::Object(o) = &ctx.this {
        if let Some(t) = ta_rec(vm, *o) {
            return Ok(Value::str(kind_name(t.kind)));
        }
    }
    Ok(Value::Undefined)
}

// ================================================================================================ prototype methods

fn callback(vm: &mut Vm, ctx: &CallCtx, name: &str) -> JsResult<Value> {
    let f = vm.arg(ctx, 0);
    if !vm.is_callable(&f) {
        return vm.throw_type(&alloc::format!("TypedArray.prototype.{}: callback is not a function", name));
    }
    Ok(f)
}

fn ta_at(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t, len) = validate(vm, &ctx.this)?;
    let a = vm.arg(ctx, 0);
    let r = vm.to_integer_or_infinity(&a)?;
    let k = if r >= 0.0 { r } else { len as f64 + r };
    if k < 0.0 || k >= len as f64 {
        return Ok(Value::Undefined);
    }
    Ok(elem(vm, &t, k as usize))
}

fn ta_copy_within(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, _, len) = validate(vm, &ctx.this)?;
    let lf = len as f64;
    let a0 = vm.arg(ctx, 0);
    let a1 = vm.arg(ctx, 1);
    let a2 = vm.arg(ctx, 2);
    let to = relative_index(vm, &a0, lf, 0.0)?;
    let from = relative_index(vm, &a1, lf, 0.0)?;
    let fin = relative_index(vm, &a2, lf, lf)?;
    let count = (fin - from).min(lf - to);
    if count > 0.0 {
        let t = ta_rec(vm, o).unwrap();
        let len = match len_of(vm, &t) {
            Some(n) => n,
            None => return vm.throw_type("TypedArray is detached or out of bounds"),
        };
        let es = t.kind.size();
        let limit = len * es + t.offset;
        let to_b = to as usize * es + t.offset;
        let from_b = from as usize * es + t.offset;
        let mut n = count as usize * es;
        if from_b >= limit || to_b >= limit {
            n = 0;
        } else {
            n = n.min(limit - from_b).min(limit - to_b);
        }
        if n > 0 {
            buffer_mut(vm, t.buffer).data.copy_within(from_b..from_b + n, to_b);
        }
    }
    Ok(Value::Object(o))
}

fn ta_entries(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, _, _) = validate(vm, &ctx.this)?;
    Ok(crate::builtins::iterator::create_array_iterator(vm, Value::Object(o), IterKind::Entries))
}
fn ta_keys(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, _, _) = validate(vm, &ctx.this)?;
    Ok(crate::builtins::iterator::create_array_iterator(vm, Value::Object(o), IterKind::Keys))
}
fn ta_values(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, _, _) = validate(vm, &ctx.this)?;
    Ok(crate::builtins::iterator::create_array_iterator(vm, Value::Object(o), IterKind::Values))
}

/// every (0) / some (1) / forEach (2)
fn walk(vm: &mut Vm, ctx: &CallCtx, mode: u8, name: &str) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let f = callback(vm, ctx, name)?;
    let this_arg = vm.arg(ctx, 1);
    for k in 0..len {
        let v = elem(vm, &t, k);
        let r = vm.call(&f, &this_arg, &[v, Value::Number(k as f64), Value::Object(o)])?;
        let b = vm.to_boolean(&r);
        match mode {
            0 if !b => return Ok(Value::Bool(false)),
            1 if b => return Ok(Value::Bool(true)),
            _ => {}
        }
    }
    Ok(match mode {
        0 => Value::Bool(true),
        1 => Value::Bool(false),
        _ => Value::Undefined,
    })
}
fn ta_every(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    walk(vm, ctx, 0, "every")
}
fn ta_some(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    walk(vm, ctx, 1, "some")
}
fn ta_for_each(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    walk(vm, ctx, 2, "forEach")
}

/// find (0) / findIndex (1) / findLast (2) / findLastIndex (3)
fn find_impl(vm: &mut Vm, ctx: &CallCtx, mode: u8, name: &str) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let f = callback(vm, ctx, name)?;
    let this_arg = vm.arg(ctx, 1);
    for i in 0..len {
        let k = if mode >= 2 { len - 1 - i } else { i };
        let v = elem(vm, &t, k);
        let r = vm.call(&f, &this_arg, &[v.clone(), Value::Number(k as f64), Value::Object(o)])?;
        if vm.to_boolean(&r) {
            return Ok(if mode % 2 == 0 { v } else { Value::Number(k as f64) });
        }
    }
    Ok(if mode % 2 == 0 { Value::Undefined } else { Value::Number(-1.0) })
}
fn ta_find(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, 0, "find")
}
fn ta_find_index(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, 1, "findIndex")
}
fn ta_find_last(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, 2, "findLast")
}
fn ta_find_last_index(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    find_impl(vm, ctx, 3, "findLastIndex")
}

fn ta_fill(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let v = vm.arg(ctx, 0);
    let nv = to_kind_numeric(vm, t.kind, &v)?;
    let lf = len as f64;
    let a1 = vm.arg(ctx, 1);
    let a2 = vm.arg(ctx, 2);
    let s = relative_index(vm, &a1, lf, 0.0)? as usize;
    let e = relative_index(vm, &a2, lf, lf)? as usize;
    let t = ta_rec(vm, o).unwrap();
    let len = match len_of(vm, &t) {
        Some(n) => n,
        None => return vm.throw_type("TypedArray is detached or out of bounds"),
    };
    let e = e.min(len);
    for k in s..e {
        set_raw(vm, t.buffer, t.offset + k * t.kind.size(), t.kind, &nv, true);
    }
    Ok(Value::Object(o))
}

fn ta_filter(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let f = callback(vm, ctx, "filter")?;
    let this_arg = vm.arg(ctx, 1);
    let mut kept = Vec::new();
    for k in 0..len {
        let v = elem(vm, &t, k);
        let r = vm.call(&f, &this_arg, &[v.clone(), Value::Number(k as f64), Value::Object(o)])?;
        if vm.to_boolean(&r) {
            kept.push(v);
        }
    }
    let a = species_create(vm, o, &[Value::Number(kept.len() as f64)])?;
    for (n, v) in kept.into_iter().enumerate() {
        vm.set_prop(a, PropertyKey::from(n as u32), v, true)?;
    }
    Ok(Value::Object(a))
}

fn ta_includes(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t, len) = validate(vm, &ctx.this)?;
    if len == 0 {
        return Ok(Value::Bool(false));
    }
    let target = vm.arg(ctx, 0);
    let fi = vm.arg(ctx, 1);
    let mut n = vm.to_integer_or_infinity(&fi)?;
    if n == f64::INFINITY {
        return Ok(Value::Bool(false));
    } else if n == f64::NEG_INFINITY {
        n = 0.0;
    }
    let start = if n >= 0.0 { n as usize } else { (len as f64 + n).max(0.0) as usize };
    for k in start..len {
        if elem(vm, &t, k).same_value_zero(&target) {
            return Ok(Value::Bool(true));
        }
    }
    Ok(Value::Bool(false))
}

fn ta_index_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t, len) = validate(vm, &ctx.this)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let target = vm.arg(ctx, 0);
    let fi = vm.arg(ctx, 1);
    let mut n = vm.to_integer_or_infinity(&fi)?;
    if n == f64::INFINITY {
        return Ok(Value::Number(-1.0));
    } else if n == f64::NEG_INFINITY {
        n = 0.0;
    }
    let start = if n >= 0.0 { n as usize } else { (len as f64 + n).max(0.0) as usize };
    for k in start..len {
        if valid_index(vm, &t, k as f64).is_none() {
            continue;
        }
        if elem(vm, &t, k).strict_eq(&target) {
            return Ok(Value::Number(k as f64));
        }
    }
    Ok(Value::Number(-1.0))
}

fn ta_last_index_of(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t, len) = validate(vm, &ctx.this)?;
    if len == 0 {
        return Ok(Value::Number(-1.0));
    }
    let target = vm.arg(ctx, 0);
    let n = if ctx.argc > 1 {
        let fi = vm.arg(ctx, 1);
        vm.to_integer_or_infinity(&fi)?
    } else {
        len as f64 - 1.0
    };
    if n == f64::NEG_INFINITY {
        return Ok(Value::Number(-1.0));
    }
    let start = if n >= 0.0 { n.min(len as f64 - 1.0) } else { len as f64 + n };
    let mut k = start;
    while k >= 0.0 {
        let ki = k as usize;
        if valid_index(vm, &t, k).is_some() && elem(vm, &t, ki).strict_eq(&target) {
            return Ok(Value::Number(k));
        }
        k -= 1.0;
    }
    Ok(Value::Number(-1.0))
}

fn ta_join(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t, len) = validate(vm, &ctx.this)?;
    let sv = vm.arg(ctx, 0);
    let sep = if sv.is_undefined() { JsStr::from_str(",") } else { vm.to_string(&sv)? };
    let mut out: Vec<u16> = Vec::new();
    for k in 0..len {
        if k > 0 {
            out.extend_from_slice(sep.units());
        }
        let v = elem(vm, &t, k);
        if !v.is_undefined() {
            let s = vm.to_string(&v)?;
            out.extend_from_slice(s.units());
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn ta_map(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let f = callback(vm, ctx, "map")?;
    let this_arg = vm.arg(ctx, 1);
    let a = species_create(vm, o, &[Value::Number(len as f64)])?;
    for k in 0..len {
        let v = elem(vm, &t, k);
        let m = vm.call(&f, &this_arg, &[v, Value::Number(k as f64), Value::Object(o)])?;
        vm.set_prop(a, PropertyKey::from(k as u32), m, true)?;
    }
    Ok(Value::Object(a))
}

fn reduce_impl(vm: &mut Vm, ctx: &CallCtx, right: bool) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let f = callback(vm, ctx, if right { "reduceRight" } else { "reduce" })?;
    if len == 0 && ctx.argc < 2 {
        return vm.throw_type("Reduce of empty array with no initial value");
    }
    let mut i = 0;
    let mut acc = if ctx.argc >= 2 {
        vm.arg(ctx, 1)
    } else {
        i = 1;
        elem(vm, &t, if right { len - 1 } else { 0 })
    };
    while i < len {
        let k = if right { len - 1 - i } else { i };
        let v = elem(vm, &t, k);
        acc = vm.call(&f, &Value::Undefined, &[acc, v, Value::Number(k as f64), Value::Object(o)])?;
        i += 1;
    }
    Ok(acc)
}
fn ta_reduce(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    reduce_impl(vm, ctx, false)
}
fn ta_reduce_right(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    reduce_impl(vm, ctx, true)
}

fn ta_reverse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let es = t.kind.size();
    let d = &mut buffer_mut(vm, t.buffer).data[t.offset..t.offset + len * es];
    let (mut lo, mut hi) = (0usize, len);
    while lo + 1 < hi {
        hi -= 1;
        for b in 0..es {
            d.swap(lo * es + b, hi * es + b);
        }
        lo += 1;
    }
    Ok(Value::Object(o))
}

fn ta_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, _) = require_ta(vm, &ctx.this)?;
    let source = vm.arg(ctx, 0);
    let off = vm.arg(ctx, 1);
    let target_offset = vm.to_integer_or_infinity(&off)?;
    if target_offset < 0.0 {
        return vm.throw_range("offset is out of bounds");
    }
    let t = ta_rec(vm, o).unwrap();
    let tlen = match len_of(vm, &t) {
        Some(n) => n,
        None => return vm.throw_type("TypedArray is detached or out of bounds"),
    };
    if let Some(st) = source.as_object().and_then(|s| ta_rec(vm, s)) {
        let slen = match len_of(vm, &st) {
            Some(n) => n,
            None => return vm.throw_type("source TypedArray is detached or out of bounds"),
        };
        if st.kind.is_bigint() != t.kind.is_bigint() {
            return vm.throw_type("Content types of source and target typed arrays differ");
        }
        if target_offset == f64::INFINITY || slen as f64 + target_offset > tlen as f64 {
            return vm.throw_range("offset is out of bounds");
        }
        let to = target_offset as usize;
        let src_bytes = buffer_data(vm, st.buffer).unwrap().data[st.offset..st.offset + slen * st.kind.size()].to_vec();
        if st.kind == t.kind {
            let start = t.offset + to * t.kind.size();
            buffer_mut(vm, t.buffer).data[start..start + src_bytes.len()].copy_from_slice(&src_bytes);
        } else {
            let ss = st.kind.size();
            for k in 0..slen {
                let v = decode(st.kind, &src_bytes[k * ss..(k + 1) * ss]);
                set_raw(vm, t.buffer, t.offset + (to + k) * t.kind.size(), t.kind, &v, true);
            }
        }
        return Ok(Value::Undefined);
    }
    let src = vm.to_object(&source)?.as_object().unwrap();
    let slen = vm.length_of(src)?;
    if target_offset == f64::INFINITY || slen + target_offset > tlen as f64 {
        return vm.throw_range("offset is out of bounds");
    }
    let to = target_offset as usize;
    for k in 0..slen as usize {
        let v = vm.get(src, &PropertyKey::from(k as u32))?;
        ta_set_index(vm, o, (to + k) as f64, &v)?;
    }
    Ok(Value::Undefined)
}

fn ta_slice(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let lf = len as f64;
    let a0 = vm.arg(ctx, 0);
    let a1 = vm.arg(ctx, 1);
    let s = relative_index(vm, &a0, lf, 0.0)? as usize;
    let mut e = relative_index(vm, &a1, lf, lf)? as usize;
    let mut count = e.saturating_sub(s);
    let a = species_create(vm, o, &[Value::Number(count as f64)])?;
    if count > 0 {
        let t = ta_rec(vm, o).unwrap();
        let len = match len_of(vm, &t) {
            Some(n) => n,
            None => return vm.throw_type("TypedArray is detached or out of bounds"),
        };
        e = e.min(len);
        count = e.saturating_sub(s);
        let at = ta_rec(vm, a).unwrap();
        if at.kind == t.kind {
            let es = t.kind.size();
            let src_start = s * es + t.offset;
            let alen = len_of(vm, &at).unwrap_or(0) * es;
            let n = (count * es).min(alen);
            if n == 0 {
                return Ok(Value::Object(a));
            }
            // Byte-by-byte, front to back (observable when the species result shares the buffer).
            for i in 0..n {
                let b = buffer_data(vm, t.buffer).unwrap().data[src_start + i];
                buffer_mut(vm, at.buffer).data[at.offset + i] = b;
            }
        } else {
            let mut n = 0u32;
            for k in s..e {
                let v = vm.get(o, &PropertyKey::from(k as u32))?;
                vm.set_prop(a, PropertyKey::from(n), v, true)?;
                n += 1;
            }
        }
    }
    let _ = t;
    Ok(Value::Object(a))
}

fn numeric_cmp(a: &Value, b: &Value) -> Ordering {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => {
            if x.is_nan() {
                return if y.is_nan() { Ordering::Equal } else { Ordering::Greater };
            }
            if y.is_nan() {
                return Ordering::Less;
            }
            if x < y {
                Ordering::Less
            } else if x > y {
                Ordering::Greater
            } else if *x == 0.0 && *y == 0.0 {
                match (x.is_sign_negative(), y.is_sign_negative()) {
                    (true, false) => Ordering::Less,
                    (false, true) => Ordering::Greater,
                    _ => Ordering::Equal,
                }
            } else {
                Ordering::Equal
            }
        }
        (Value::BigInt(x), Value::BigInt(y)) => crate::builtins::bigint::cmp(x, y),
        _ => Ordering::Equal,
    }
}

fn sorted_values(vm: &mut Vm, t: &Ta, len: usize, cmp: &Value) -> JsResult<Vec<Value>> {
    let mut items: Vec<Value> = (0..len).map(|k| elem(vm, t, k)).collect();
    if cmp.is_undefined() {
        items.sort_by(numeric_cmp);
    } else {
        crate::builtins::array::sort_values(vm, &mut items, cmp)?;
    }
    Ok(items)
}

fn ta_sort(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cmp = vm.arg(ctx, 0);
    if !cmp.is_undefined() && !vm.is_callable(&cmp) {
        return vm.throw_type("The comparison function must be either a function or undefined");
    }
    let (o, t, len) = validate(vm, &ctx.this)?;
    let items = sorted_values(vm, &t, len, &cmp)?;
    for (k, v) in items.iter().enumerate() {
        let t = ta_rec(vm, o).unwrap();
        if let Some(idx) = valid_index(vm, &t, k as f64) {
            set_raw(vm, t.buffer, t.offset + idx * t.kind.size(), t.kind, v, true);
        }
    }
    Ok(Value::Object(o))
}

fn ta_to_sorted(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let cmp = vm.arg(ctx, 0);
    if !cmp.is_undefined() && !vm.is_callable(&cmp) {
        return vm.throw_type("The comparison function must be either a function or undefined");
    }
    let (_, t, len) = validate(vm, &ctx.this)?;
    let a = create_same_type(vm, &t, len)?;
    let items = sorted_values(vm, &t, len, &cmp)?;
    let at = ta_rec(vm, a).unwrap();
    for (k, v) in items.iter().enumerate() {
        set_raw(vm, at.buffer, at.offset + k * at.kind.size(), at.kind, v, true);
    }
    Ok(Value::Object(a))
}

fn ta_to_reversed(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t, len) = validate(vm, &ctx.this)?;
    let a = create_same_type(vm, &t, len)?;
    let at = ta_rec(vm, a).unwrap();
    for k in 0..len {
        let v = elem(vm, &t, len - 1 - k);
        set_raw(vm, at.buffer, at.offset + k * at.kind.size(), at.kind, &v, true);
    }
    Ok(Value::Object(a))
}

fn ta_with(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, t, len) = validate(vm, &ctx.this)?;
    let iv = vm.arg(ctx, 0);
    let rel = vm.to_integer_or_infinity(&iv)? + 0.0;
    let actual = if rel >= 0.0 { rel } else { len as f64 + rel };
    let v = vm.arg(ctx, 1);
    let nv = to_kind_numeric(vm, t.kind, &v)?;
    if !ta_valid_index(vm, o, actual) {
        return vm.throw_range("Invalid typed array index");
    }
    let a = create_same_type(vm, &t, len)?;
    let at = ta_rec(vm, a).unwrap();
    for k in 0..len {
        let fv = if k as f64 == actual { nv.clone() } else { elem(vm, &t, k) };
        let fv = to_kind_numeric(vm, at.kind, &fv)?;
        set_raw(vm, at.buffer, at.offset + k * at.kind.size(), at.kind, &fv, true);
    }
    Ok(Value::Object(a))
}

fn ta_subarray(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, t) = require_ta(vm, &ctx.this)?;
    let src_len = len_of(vm, &t).unwrap_or(0) as f64;
    let a0 = vm.arg(ctx, 0);
    let a1 = vm.arg(ctx, 1);
    let s = relative_index(vm, &a0, src_len, 0.0)?;
    let es = t.kind.size();
    let begin = t.offset + s as usize * es;
    let args = if t.length.is_none() && a1.is_undefined() {
        alloc::vec![Value::Object(t.buffer), Value::Number(begin as f64)]
    } else {
        let e = relative_index(vm, &a1, src_len, src_len)?;
        let n = (e - s).max(0.0);
        alloc::vec![Value::Object(t.buffer), Value::Number(begin as f64), Value::Number(n)]
    };
    let r = species_create(vm, o, &args)?;
    Ok(Value::Object(r))
}

fn ta_to_locale_string(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (_, t, len) = validate(vm, &ctx.this)?;
    let mut out: Vec<u16> = Vec::new();
    for k in 0..len {
        if k > 0 {
            out.push(b',' as u16);
        }
        let v = elem(vm, &t, k);
        if !v.is_nullish() {
            let r = vm.invoke(&v, &PropertyKey::from_str("toLocaleString"), &[])?;
            let s = vm.to_string(&r)?;
            out.extend_from_slice(s.units());
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

// ================================================================================================ DataView

fn dv_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Constructor DataView requires 'new'");
    }
    let b = vm.arg(ctx, 0);
    let buf = match &b {
        Value::Object(o) if buffer_data(vm, *o).is_some() => *o,
        _ => return vm.throw_type("First argument to DataView constructor must be an ArrayBuffer"),
    };
    let bo = vm.arg(ctx, 1);
    let offset = vm.to_index(&bo)?;
    if is_detached(vm, buf) {
        return vm.throw_type("Cannot construct a DataView on a detached ArrayBuffer");
    }
    let bl = buf_len(vm, buf);
    if offset > bl {
        return vm.throw_range("Start offset is outside the bounds of the buffer");
    }
    let fixed = is_fixed_buffer(vm, buf);
    let lv = vm.arg(ctx, 2);
    let view_len = if lv.is_undefined() {
        if fixed { Some(bl - offset) } else { None }
    } else {
        let l = vm.to_index(&lv)?;
        if offset as u128 + l as u128 > bl as u128 {
            return vm.throw_range("Invalid DataView length");
        }
        Some(l)
    };
    let nt = ctx.new_target.clone();
    let proto = vm.get_prototype_from_ctor(&nt, |i| i.data_view_proto)?;
    if is_detached(vm, buf) {
        return vm.throw_type("Cannot construct a DataView on a detached ArrayBuffer");
    }
    let bl = buf_len(vm, buf);
    if offset > bl {
        return vm.throw_range("Start offset is outside the bounds of the buffer");
    }
    if !lv.is_undefined() {
        if let Some(l) = view_len {
            if offset + l > bl {
                return vm.throw_range("Invalid DataView length");
            }
        }
    }
    let o = vm.alloc(ObjectData::new(Some(proto), Kind::DataView(Box::new(DataViewData { buffer: buf, byte_offset: offset, byte_length: view_len }))));
    Ok(Value::Object(o))
}

fn this_dv(vm: &mut Vm, v: &Value) -> JsResult<(Obj, usize, Option<usize>)> {
    if let Value::Object(o) = v {
        if let Kind::DataView(d) = &vm.heap.get(*o).kind {
            return Ok((d.buffer, d.byte_offset, d.byte_length));
        }
    }
    vm.throw_type("Receiver is not a DataView")
}

/// GetViewByteLength, or None when IsViewOutOfBounds.
fn dv_len(vm: &Vm, buf: Obj, off: usize, len: Option<usize>) -> Option<usize> {
    if is_detached(vm, buf) {
        return None;
    }
    let bl = buf_len(vm, buf);
    let end = match len {
        None => bl,
        Some(n) => off + n,
    };
    if off > bl || end > bl {
        return None;
    }
    Some(end - off)
}

fn dv_buffer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (b, _, _) = this_dv(vm, &ctx.this)?;
    Ok(Value::Object(b))
}
fn dv_byte_length(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (b, o, l) = this_dv(vm, &ctx.this)?;
    match dv_len(vm, b, o, l) {
        Some(n) => Ok(Value::Number(n as f64)),
        None => vm.throw_type("DataView is detached or out of bounds"),
    }
}
fn dv_byte_offset(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (b, o, l) = this_dv(vm, &ctx.this)?;
    match dv_len(vm, b, o, l) {
        Some(_) => Ok(Value::Number(o as f64)),
        None => vm.throw_type("DataView is detached or out of bounds"),
    }
}

fn dv_kind(vm: &Vm, ctx: &CallCtx) -> TAKind {
    match vm.native_slot(ctx.callee, 0) {
        Value::Number(n) => KINDS[n as usize],
        _ => TAKind::Uint8,
    }
}

fn dv_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let kind = dv_kind(vm, ctx);
    let this = ctx.this.clone();
    this_dv(vm, &this)?;
    let ri = vm.arg(ctx, 0);
    let idx = vm.to_index(&ri)?;
    let le = vm.arg(ctx, 1);
    let little = vm.to_boolean(&le);
    let (b, o, l) = this_dv(vm, &this)?;
    let size = match dv_len(vm, b, o, l) {
        Some(n) => n,
        None => return vm.throw_type("DataView is detached or out of bounds"),
    };
    if idx as u128 + kind.size() as u128 > size as u128 {
        return vm.throw_range("Offset is outside the bounds of the DataView");
    }
    Ok(get_raw(vm, b, o + idx, kind, little))
}

fn dv_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let kind = dv_kind(vm, ctx);
    let this = ctx.this.clone();
    this_dv(vm, &this)?;
    let ri = vm.arg(ctx, 0);
    let idx = vm.to_index(&ri)?;
    let v = vm.arg(ctx, 1);
    let nv = to_kind_numeric(vm, kind, &v)?;
    let le = vm.arg(ctx, 2);
    let little = vm.to_boolean(&le);
    let (b, o, l) = this_dv(vm, &this)?;
    let size = match dv_len(vm, b, o, l) {
        Some(n) => n,
        None => return vm.throw_type("DataView is detached or out of bounds"),
    };
    if idx as u128 + kind.size() as u128 > size as u128 {
        return vm.throw_range("Offset is outside the bounds of the DataView");
    }
    set_raw(vm, b, o + idx, kind, &nv, little);
    Ok(Value::Undefined)
}

// ================================================================================================ Atomics

/// ValidateIntegerTypedArray + ValidateAtomicAccess: (array, record, byte index in buffer).
fn atomic_access(vm: &mut Vm, ta: &Value, index: &Value, waitable: bool) -> JsResult<(Obj, Ta, usize)> {
    let (o, t, len) = validate(vm, ta)?;
    let ok = if waitable {
        matches!(t.kind, TAKind::Int32 | TAKind::BigInt64)
    } else {
        !matches!(t.kind, TAKind::Float16 | TAKind::Float32 | TAKind::Float64 | TAKind::Uint8Clamped)
    };
    if !ok {
        return vm.throw_type("Atomics operation on an unsupported TypedArray type");
    }
    let i = vm.to_index(index)?;
    if i >= len {
        return vm.throw_range("Atomics access index out of range");
    }
    Ok((o, t, i * t.kind.size() + t.offset))
}

/// RevalidateAtomicAccess
fn revalidate(vm: &mut Vm, o: Obj, byte_index: usize) -> JsResult<Ta> {
    let t = ta_rec(vm, o).unwrap();
    let len = match len_of(vm, &t) {
        Some(n) => n,
        None => return vm.throw_type("TypedArray is detached or out of bounds"),
    };
    if byte_index >= t.offset + len * t.kind.size() {
        return vm.throw_range("Atomics access index out of range");
    }
    Ok(t)
}

fn atomic_operand(vm: &mut Vm, kind: TAKind, v: &Value) -> JsResult<Value> {
    if kind.is_bigint() {
        Ok(Value::BigInt(vm.to_bigint(v)?))
    } else {
        let n = vm.to_integer_or_infinity(v)?;
        Ok(Value::Number(n + 0.0))
    }
}

fn rmw(vm: &mut Vm, ctx: &CallCtx, op: fn(u64, u64) -> u64) -> JsResult<Value> {
    let ta = vm.arg(ctx, 0);
    let idx = vm.arg(ctx, 1);
    let (o, t, bi) = atomic_access(vm, &ta, &idx, false)?;
    let v = vm.arg(ctx, 2);
    let operand = atomic_operand(vm, t.kind, &v)?;
    let t = revalidate(vm, o, bi)?;
    let old = get_raw(vm, t.buffer, bi, t.kind, true);
    let n = t.kind.size();
    let mut ob = [0u8; 8];
    ob[..n].copy_from_slice(&buffer_data(vm, t.buffer).unwrap().data[bi..bi + n]);
    let a = u64::from_le_bytes(ob);
    let b = u64::from_le_bytes(encode(t.kind, &operand));
    let r = op(a, b).to_le_bytes();
    buffer_mut(vm, t.buffer).data[bi..bi + n].copy_from_slice(&r[..n]);
    Ok(old)
}

fn atomics_add(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rmw(vm, ctx, |a, b| a.wrapping_add(b))
}
fn atomics_sub(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rmw(vm, ctx, |a, b| a.wrapping_sub(b))
}
fn atomics_and(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rmw(vm, ctx, |a, b| a & b)
}
fn atomics_or(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rmw(vm, ctx, |a, b| a | b)
}
fn atomics_xor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rmw(vm, ctx, |a, b| a ^ b)
}
fn atomics_exchange(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    rmw(vm, ctx, |_, b| b)
}

fn atomics_compare_exchange(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ta = vm.arg(ctx, 0);
    let idx = vm.arg(ctx, 1);
    let (o, t, bi) = atomic_access(vm, &ta, &idx, false)?;
    let e = vm.arg(ctx, 2);
    let r = vm.arg(ctx, 3);
    let expected = atomic_operand(vm, t.kind, &e)?;
    let replacement = atomic_operand(vm, t.kind, &r)?;
    let t = revalidate(vm, o, bi)?;
    let n = t.kind.size();
    let old = get_raw(vm, t.buffer, bi, t.kind, true);
    let eb = encode(t.kind, &expected);
    let cur = buffer_data(vm, t.buffer).unwrap().data[bi..bi + n].to_vec();
    if cur[..] == eb[..n] {
        let rb = encode(t.kind, &replacement);
        buffer_mut(vm, t.buffer).data[bi..bi + n].copy_from_slice(&rb[..n]);
    }
    Ok(old)
}

fn atomics_load(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ta = vm.arg(ctx, 0);
    let idx = vm.arg(ctx, 1);
    let (o, _, bi) = atomic_access(vm, &ta, &idx, false)?;
    let t = revalidate(vm, o, bi)?;
    Ok(get_raw(vm, t.buffer, bi, t.kind, true))
}

fn atomics_store(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ta = vm.arg(ctx, 0);
    let idx = vm.arg(ctx, 1);
    let (o, t, bi) = atomic_access(vm, &ta, &idx, false)?;
    let v = vm.arg(ctx, 2);
    let operand = atomic_operand(vm, t.kind, &v)?;
    let t = revalidate(vm, o, bi)?;
    set_raw(vm, t.buffer, bi, t.kind, &operand, true);
    Ok(operand)
}

fn atomics_is_lock_free(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(ctx, 0);
    let n = vm.to_integer_or_infinity(&v)?;
    Ok(Value::Bool(n == 1.0 || n == 2.0 || n == 4.0 || n == 8.0))
}

fn atomics_notify(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ta = vm.arg(ctx, 0);
    let idx = vm.arg(ctx, 1);
    let (o, t, bi) = atomic_access(vm, &ta, &idx, true)?;
    let c = vm.arg(ctx, 2);
    if !c.is_undefined() {
        vm.to_integer_or_infinity(&c)?;
    }
    let _ = (o, bi, t);
    // A single agent never has waiters.
    Ok(Value::Number(0.0))
}

fn atomics_wait(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ta = vm.arg(ctx, 0);
    let idx = vm.arg(ctx, 1);
    let (_, t0, _) = validate(vm, &ta)?;
    if !matches!(t0.kind, TAKind::Int32 | TAKind::BigInt64) {
        return vm.throw_type("Atomics operation on an unsupported TypedArray type");
    }
    if !buffer_data(vm, t0.buffer).map(|d| d.shared).unwrap_or(false) {
        return vm.throw_type("Atomics.wait requires a shared typed array");
    }
    let (o, t, bi) = atomic_access(vm, &ta, &idx, true)?;
    let v = vm.arg(ctx, 2);
    let value = if t.kind.is_bigint() { Value::BigInt(vm.to_bigint(&v)?) } else { Value::Number(vm.to_int32(&v)? as f64) };
    let tv = vm.arg(ctx, 3);
    let q = vm.to_number(&tv)?;
    let _timeout = if q.is_nan() { f64::INFINITY } else { q.max(0.0) };
    let _ = (o, bi, value);
    // AgentCanSuspend(): the host's main agent has [[CanBlock]] false.
    vm.throw_type("Atomics.wait cannot be called in this context")
}
