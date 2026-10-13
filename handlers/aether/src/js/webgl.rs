//! CHARTER: Aether (CODEX §2, Web) — seam: handler-internal (Ring 3 host) over the shared-core `gl_core`.
//!
//! WEBGL1 (LEDGER SR136): the WebIDL face of WebGL 1.0 over `gl_core` (UnaOS's software OpenGL ES 2.0 —
//! no system GL, no ANGLE, R83/R104).
//!
//! Interfaces: `WebGLRenderingContext` (every method and attribute of the WebGL 1.0 IDL; the constants are
//! gl_core's IDL table, installed on the interface object and its prototype), `WebGLObject` and its six
//! children (`WebGLBuffer`, `WebGLFramebuffer`, `WebGLProgram`, `WebGLRenderbuffer`, `WebGLShader`,
//! `WebGLTexture`), `WebGLUniformLocation`, `WebGLActiveInfo`, `WebGLShaderPrecisionFormat` and
//! `WebGLContextEvent`. Extensions: OES_element_index_uint, OES_standard_derivatives, WEBGL_lose_context
//! (refused by name, ledgered `webgl-extension-refused:<name>`: every other, OES_texture_float among them).
//!
//! A context is a branded object holding its surface id (the GL context itself lives in `crate::canvas` beside
//! the canvas bitmap — the surface hand-off); each GL object is ONE wrapper per (surface, kind, name), so
//! `getParameter(ARRAY_BUFFER_BINDING)` is the very object `createBuffer` returned. While the context is lost
//! every method answers its lost-context default (WebGL 1.0 "lose the context") without reaching the core.

use super::dom::{self, attr_value, iface, register_iface, with_doc};
use super::idl::*;
use super::{page, TargetKey};
use crate::canvas;
use gl_core::consts as c;
use gl_core::program::UKind;
use gl_core::texture::Pixels;
use gl_core::{Attributes, Context as Gl, ObjKind, Param, UniformLoc};
use html_core::NodeId;
use js_core::builtins::typedarray::{new_array_buffer, ta_length, KINDS};
use js_core::vm::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

const T_GLCTX: u8 = 120;
const T_GLOBJ: u8 = 121;
const T_GLLOC: u8 = 122;
const T_GLINFO: u8 = 123;
const T_GLPREC: u8 = 124;
const T_GLEXT: u8 = 125;

/// `[SameObject]` slot of an element's WebGL context (canvas.rs's 2D context is 200).
pub const SO_GL: u8 = 201;

/// js/canvas.rs's platform-object tags of the image sources texImage2D reads (canvas.rs:36, :39).
const T_IDATA_CANVAS: u8 = 64;
const T_BMP_CANVAS: u8 = 67;

/// js/events.rs's event flag bits (events.rs:24-33) a constructed WebGLContextEvent adjusts.
const F_COMPOSED: u32 = 4;
const F_TRUSTED: u32 = 512;

/// The extensions offered, in getSupportedExtensions order.
const EXTENSIONS: &[&str] = &["OES_element_index_uint", "OES_standard_derivatives", "WEBGL_lose_context"];

const OBJ_IFACES: [&str; 6] = ["WebGLBuffer", "WebGLTexture", "WebGLRenderbuffer", "WebGLFramebuffer", "WebGLProgram", "WebGLShader"];

fn kind_index(k: ObjKind) -> u8 {
    match k {
        ObjKind::Buffer => 0,
        ObjKind::Texture => 1,
        ObjKind::Renderbuffer => 2,
        ObjKind::Framebuffer => 3,
        ObjKind::Program => 4,
        ObjKind::Shader => 5,
    }
}

#[derive(Default)]
struct GlState {
    /// (surface, kind, name) → host root of the wrapper.
    objs: HashMap<(u32, u8, u32), usize>,
    /// surface → the lost context may be restored (its webglcontextlost was canceled).
    restorable: HashMap<u32, bool>,
}

thread_local! {
    static STATE: RefCell<GlState> = RefCell::new(GlState::default());
}

// ------------------------------------------------------------------------------------------------ helpers

fn gl<R: Default>(sid: u32, f: impl FnOnce(&mut Gl) -> R) -> R {
    canvas::with_gl(sid, f).unwrap_or_default()
}

fn is_lost(sid: u32) -> bool {
    canvas::with_gl(sid, |g| g.lost).unwrap_or(true)
}

fn throw_illegal<T>(vm: &mut Vm) -> JsResult<T> {
    vm.throw_type("Illegal invocation")
}

fn a_u32(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<u32> {
    let v = arg(vm, ctx, i);
    vm.to_uint32(&v)
}

fn a_i32(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<i32> {
    let v = arg(vm, ctx, i);
    vm.to_int32(&v)
}

fn a_f32(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<f32> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_number(&v)? as f32)
}

/// WebIDL `long long` (GLintptr / GLsizeiptr).
fn a_i64(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<i64> {
    let v = arg(vm, ctx, i);
    let n = vm.to_number(&v)?;
    Ok(if n.is_finite() { n.trunc().clamp(-9.007e15, 9.007e15) as i64 } else { 0 })
}

fn a_bool(vm: &Vm, ctx: &CallCtx, i: usize) -> bool {
    boolean(vm, &arg(vm, ctx, i))
}

fn canvas_dims(el: usize) -> (u32, u32) {
    let dim = |name: &str, d: u32| -> u32 {
        let Some(v) = attr_value(el, name) else { return d };
        let t = v.trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
        let t = t.strip_prefix('+').unwrap_or(t);
        let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
        match digits.parse::<u64>() {
            Ok(n) if n <= 2_147_483_647 => n as u32,
            _ => d,
        }
    };
    (dim("width", 300), dim("height", 150))
}

fn this_gl(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(Obj, u32)> {
    let Some(o) = this_tagged(vm, &ctx.this, T_GLCTX) else { return throw_illegal(vm) };
    Ok((o, slot_num(vm, o, 0) as u32))
}

/// The wrapper of GL object `name` of `kind` (one per object).
fn wrap_obj(vm: &mut Vm, sid: u32, kind: ObjKind, name: u32) -> Value {
    if name == 0 {
        return Value::Null;
    }
    let k = kind_index(kind);
    if let Some(r) = STATE.with(|s| s.borrow().objs.get(&(sid, k, name)).copied()) {
        return rooted(vm, r);
    }
    let proto = iface(OBJ_IFACES[k as usize]).map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let o = host_obj(vm, proto, T_GLOBJ, vec![num(sid as f64), num(k as f64), num(name as f64)]);
    let r = root(vm, Value::Object(o));
    STATE.with(|s| s.borrow_mut().objs.insert((sid, k, name), r));
    Value::Object(o)
}

/// An object argument: Ok(Some(Some(name))), Ok(Some(None)) for null (nullable parameters only), Ok(None)
/// when it belongs to another context (INVALID_OPERATION already recorded), Err = TypeError.
fn obj_arg(vm: &mut Vm, ctx: &CallCtx, i: usize, sid: u32, kind: ObjKind, nullable: bool, method: &str) -> JsResult<Option<Option<u32>>> {
    let v = arg(vm, ctx, i);
    if v.is_null() || (v.is_undefined() && nullable) {
        if nullable {
            return Ok(Some(None));
        }
    }
    let k = kind_index(kind);
    let ok = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_GLOBJ) && slot_num(vm, *o, 1) as u8 == k);
    let Some(o) = ok else {
        return vm.throw_type(&format!("Failed to execute '{method}' on 'WebGLRenderingContext': parameter {} is not of type '{}'.", i + 1, OBJ_IFACES[k as usize]));
    };
    if slot_num(vm, o, 0) as u32 != sid {
        gl(sid, |g| g.err(c::INVALID_OPERATION));
        return Ok(None);
    }
    Ok(Some(Some(slot_num(vm, o, 2) as u32)))
}

fn loc_arg(vm: &mut Vm, ctx: &CallCtx, i: usize, sid: u32, method: &str) -> JsResult<Option<Option<UniformLoc>>> {
    let v = arg(vm, ctx, i);
    if v.is_nullish() {
        return Ok(Some(None));
    }
    let Some(o) = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_GLLOC)) else {
        return vm.throw_type(&format!("Failed to execute '{method}' on 'WebGLRenderingContext': parameter {} is not of type 'WebGLUniformLocation'.", i + 1));
    };
    if slot_num(vm, o, 0) as u32 != sid {
        gl(sid, |g| g.err(c::INVALID_OPERATION));
        return Ok(None);
    }
    Ok(Some(Some(UniformLoc { program: slot_num(vm, o, 1) as u32, link_gen: slot_num(vm, o, 2) as u32, index: slot_num(vm, o, 3) as u32 })))
}

fn typed_array(vm: &mut Vm, kind: TAKind, bytes: Vec<u8>) -> JsResult<Value> {
    let len = bytes.len() / kind.size();
    let b = new_array_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(b).kind {
        d.data = bytes;
    }
    let ki = KINDS.iter().position(|k| *k == kind).unwrap_or(0);
    let proto = vm.intr().ta_protos[ki];
    Ok(Value::Object(vm.alloc(ObjectData::new(Some(proto), Kind::TypedArray(Box::new(TypedArrayData { kind, buffer: b, byte_offset: 0, length: Some(len) }))))))
}

fn param_value(vm: &mut Vm, sid: u32, p: Param) -> JsResult<Value> {
    Ok(match p {
        Param::Null => Value::Null,
        Param::Bool(b) => Value::Bool(b),
        Param::Int(i) => num(i as f64),
        Param::Float(f) => num(f),
        Param::Str(s) => super::idl::s(&s),
        Param::F32s(v) => typed_array(vm, TAKind::Float32, v.iter().flat_map(|x| x.to_le_bytes()).collect())?,
        Param::I32s(v) => typed_array(vm, TAKind::Int32, v.iter().flat_map(|x| x.to_le_bytes()).collect())?,
        Param::U32s(v) => typed_array(vm, TAKind::Uint32, v.iter().flat_map(|x| x.to_le_bytes()).collect())?,
        Param::Bools(v) => Value::Object(vm.new_array(v.into_iter().map(Value::Bool).collect())),
        Param::Object(k, n) => wrap_obj(vm, sid, k, n),
    })
}

/// A typed array's element kind and bytes (its view, honouring offset and length).
fn view(vm: &Vm, v: &Value) -> Option<(TAKind, Vec<u8>)> {
    let o = v.as_object()?;
    let kind = match &vm.heap.get(o).kind {
        Kind::TypedArray(t) => t.kind,
        _ => return None,
    };
    Some((kind, super::net::buffer_bytes(vm, v)?))
}

/// `Float32Array or sequence<GLfloat>` (and the Int32Array / sequence<long> form): the numbers.
fn number_seq(vm: &mut Vm, v: &Value, what: &str) -> JsResult<Vec<f64>> {
    if let Some((k, b)) = view(vm, v) {
        let n = k.size();
        let mut out = Vec::with_capacity(b.len() / n.max(1));
        for ch in b.chunks_exact(n) {
            out.push(match k {
                TAKind::Int8 => ch[0] as i8 as f64,
                TAKind::Uint8 | TAKind::Uint8Clamped => ch[0] as f64,
                TAKind::Int16 => i16::from_le_bytes([ch[0], ch[1]]) as f64,
                TAKind::Uint16 => u16::from_le_bytes([ch[0], ch[1]]) as f64,
                TAKind::Int32 => i32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]) as f64,
                TAKind::Uint32 => u32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]) as f64,
                TAKind::Float32 => f32::from_le_bytes([ch[0], ch[1], ch[2], ch[3]]) as f64,
                TAKind::Float64 => f64::from_le_bytes([ch[0], ch[1], ch[2], ch[3], ch[4], ch[5], ch[6], ch[7]]),
                _ => 0.0,
            });
        }
        return Ok(out);
    }
    let Some(o) = v.as_object() else { return vm.throw_type(&format!("The provided value is not of type '{what}'.")) };
    let len = get(vm, o, "length")?;
    let n = vm.to_number(&len)?;
    let n = if n.is_finite() && n > 0.0 { n.min(16_777_216.0) as usize } else { 0 };
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let e = vm.get(o, &PropertyKey::from_str(&i.to_string()))?;
        out.push(vm.to_number(&e)?);
    }
    Ok(out)
}

fn info_obj(vm: &mut Vm, i: gl_core::ActiveInfo) -> Value {
    let proto = iface("WebGLActiveInfo").map(|x| x.proto).unwrap_or_else(|| vm.intr().object_proto);
    Value::Object(host_obj(vm, proto, T_GLINFO, vec![num(i.size as f64), num(i.ty as f64), super::idl::s(&i.name)]))
}

fn info_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_GLINFO) else { return throw_illegal(vm) };
    let i: usize = callee_str(vm, ctx).parse().unwrap_or(0);
    Ok(slot(vm, o, i))
}

fn prec_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_GLPREC) else { return throw_illegal(vm) };
    let i: usize = callee_str(vm, ctx).parse().unwrap_or(0);
    Ok(slot(vm, o, i))
}

// ------------------------------------------------------------------------------------------------ image sources

/// A TexImageSource as straight RGBA8 (WebGL 1.0 texImage2D's ImageData / HTMLImageElement /
/// HTMLCanvasElement / HTMLVideoElement / ImageBitmap). Ok(None) = not available (nothing uploaded).
fn image_source(vm: &mut Vm, v: &Value) -> JsResult<Option<(u32, u32, Rc<Vec<u8>>)>> {
    let type_err = |vm: &mut Vm| -> JsResult<Option<(u32, u32, Rc<Vec<u8>>)>> {
        vm.throw_type("Failed to execute 'texImage2D' on 'WebGLRenderingContext': The provided value is not of type '(ImageBitmap or ImageData or HTMLImageElement or HTMLCanvasElement or HTMLVideoElement)'.")
    };
    let Some(o) = v.as_object() else { return type_err(vm) };
    if let Some(el) = dom::node_of(vm, v) {
        let tag = with_doc(|d| d.element(NodeId(el)).map(|e| e.local.clone()));
        match tag.as_deref() {
            Some("img") => {
                let src = attr_value(el, "src").and_then(|x| dom::resolve_url(dom::node_document(el), &x));
                return Ok(src.and_then(|u| crate::images::get(&u)).map(|img| (img.width(), img.height(), Rc::new(img.as_raw().clone()))));
            }
            Some("canvas") => {
                let (w, h) = canvas_dims(el);
                let snap = canvas::element_surface(el).and_then(|id| canvas::with(id, |c| c.snapshot()));
                return Ok(Some(match snap {
                    Some(b) => (b.w as u32, b.h as u32, b.rgba),
                    None => (w, h, Rc::new(vec![0u8; (w as usize) * (h as usize) * 4])),
                }));
            }
            Some("video") => {
                let node = dom::node_ref(el);
                let pic = crate::media::paint_for(&node).and_then(|p| p.picture);
                return Ok(pic.map(|img| (img.width(), img.height(), Rc::new(img.as_raw().clone()))));
            }
            _ => return type_err(vm),
        }
    }
    match tag_of(vm, o) {
        Some(T_IDATA_CANVAS) => {
            let (w, h) = (slot_num(vm, o, 0) as u32, slot_num(vm, o, 1) as u32);
            let data = slot(vm, o, 2);
            let bytes = super::net::buffer_bytes(vm, &data).unwrap_or_default();
            Ok(Some((w, h, Rc::new(bytes))))
        }
        Some(T_BMP_CANVAS) => {
            let (w, h) = (slot_num(vm, o, 0) as u32, slot_num(vm, o, 1) as u32);
            let buf = slot(vm, o, 2);
            match buf.as_object().map(|b| &vm.heap.get(b).kind) {
                Some(Kind::ArrayBuffer(d)) => Ok(Some((w, h, Rc::new(d.data.clone())))),
                _ => throw_dom(vm, "InvalidStateError", "Failed to execute 'texImage2D' on 'WebGLRenderingContext': The image source is detached."),
            }
        }
        _ => type_err(vm),
    }
}

/// An ArrayBufferView upload's bytes, its element type checked against `ty` (WebGL 1.0: Uint8Array for
/// UNSIGNED_BYTE, Uint16Array for the packed types; a mismatch is INVALID_OPERATION → Ok(None)).
fn pixels_view(vm: &mut Vm, v: &Value, ty: u32, sid: u32, method: &str) -> JsResult<Option<Option<Vec<u8>>>> {
    if v.is_nullish() {
        return Ok(Some(None));
    }
    let Some((k, b)) = view(vm, v) else {
        return vm.throw_type(&format!("Failed to execute '{method}' on 'WebGLRenderingContext': parameter is not of type 'ArrayBufferView'."));
    };
    let ok = match ty {
        c::UNSIGNED_BYTE => matches!(k, TAKind::Uint8 | TAKind::Uint8Clamped),
        c::UNSIGNED_SHORT_5_6_5 | c::UNSIGNED_SHORT_4_4_4_4 | c::UNSIGNED_SHORT_5_5_5_1 => k == TAKind::Uint16,
        _ => true,
    };
    if !ok {
        gl(sid, |g| g.err(c::INVALID_OPERATION));
        return Ok(None);
    }
    Ok(Some(Some(b)))
}

// ------------------------------------------------------------------------------------------------ context creation

/// `getContext('webgl' | 'experimental-webgl', attributes)` for a canvas element (called from
/// js/canvas.rs's getContext arm once it has refused a canvas that holds a 2D context).
pub fn element_context(vm: &mut Vm, el: usize, attrs: &Value) -> JsResult<Value> {
    if let Some(o) = page(|p| p.same_object.get(&(el, SO_GL)).map(|x| x.0)) {
        return Ok(Value::Object(o));
    }
    let mut a = Attributes::default();
    if attrs.is_object() {
        for (name, field) in [("alpha", 0), ("depth", 1), ("stencil", 2), ("antialias", 3), ("premultipliedAlpha", 4), ("preserveDrawingBuffer", 5), ("failIfMajorPerformanceCaveat", 6), ("desynchronized", 7)] {
            let v = dict_member(vm, attrs, name)?;
            if v.is_undefined() {
                continue;
            }
            let b = boolean(vm, &v);
            match field {
                0 => a.alpha = b,
                1 => a.depth = b,
                2 => a.stencil = b,
                3 => a.antialias = b,
                4 => a.premultiplied_alpha = b,
                5 => a.preserve_drawing_buffer = b,
                6 => a.fail_if_major_performance_caveat = b,
                _ => a.desynchronized = b,
            }
        }
        let p = dict_member(vm, attrs, "powerPreference")?;
        if !p.is_undefined() {
            let t = string(vm, &p)?;
            a.power_preference = match t.as_str() {
                "default" => 0,
                "low-power" => 1,
                "high-performance" => 2,
                _ => return vm.throw_type(&format!("Failed to execute 'getContext': The provided value '{t}' is not a valid enum value of type WebGLPowerPreference.")),
            };
        }
    }
    let (w, h) = canvas_dims(el);
    let sid = canvas::create(w as usize, h as usize, !a.alpha);
    canvas::attach_gl(sid, Gl::new(w, h, a));
    canvas::bind_element(el, sid);
    let wrapper = dom::wrap(vm, el);
    let proto = iface("WebGLRenderingContext").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let exts = vm.new_plain_object();
    let o = host_obj(vm, proto, T_GLCTX, vec![num(sid as f64), wrapper, Value::Object(exts)]);
    let r = root(vm, Value::Object(o));
    page(|p| p.same_object.insert((el, SO_GL), (o, r)));
    Ok(Value::Object(o))
}

fn context_attributes(vm: &mut Vm, sid: u32) -> JsResult<Value> {
    let Some(a) = canvas::with_gl(sid, |g| g.attrs) else { return Ok(Value::Null) };
    let o = vm.new_plain_object();
    for (n, v) in [
        ("alpha", a.alpha),
        ("antialias", a.antialias),
        ("depth", a.depth),
        ("desynchronized", a.desynchronized),
        ("failIfMajorPerformanceCaveat", a.fail_if_major_performance_caveat),
        ("premultipliedAlpha", a.premultiplied_alpha),
        ("preserveDrawingBuffer", a.preserve_drawing_buffer),
        ("stencil", a.stencil),
    ] {
        vm.create_data_property(o, PropertyKey::from_str(n), Value::Bool(v))?;
    }
    let pp = ["default", "low-power", "high-performance"][a.power_preference.min(2) as usize];
    vm.create_data_property(o, PropertyKey::from_str("powerPreference"), super::idl::s(pp))?;
    Ok(Value::Object(o))
}

// ------------------------------------------------------------------------------------------------ extensions

fn get_extension(vm: &mut Vm, o: Obj, sid: u32, name: &str) -> JsResult<Value> {
    let Some(canon) = EXTENSIONS.iter().find(|e| e.eq_ignore_ascii_case(name)).copied() else {
        crate::ledger::record_dom(&format!("webgl-extension-refused:{}", super::clip(name, 40)));
        return Ok(Value::Null);
    };
    let exts = slot(vm, o, 2).as_object().unwrap();
    let have = get(vm, exts, canon)?;
    if have.is_object() {
        return Ok(have);
    }
    let op = vm.intr().object_proto;
    let eo = host_obj(vm, op, T_GLEXT, vec![num(sid as f64), Value::Object(o)]);
    match canon {
        "OES_element_index_uint" => gl(sid, |g| g.ext.element_index_uint = true),
        "OES_standard_derivatives" => {
            gl(sid, |g| g.ext.standard_derivatives = true);
            data(vm, eo, "FRAGMENT_SHADER_DERIVATIVE_HINT_OES", num(c::FRAGMENT_SHADER_DERIVATIVE_HINT_OES as f64), E);
        }
        _ => {
            op_with(vm, eo, "loseContext", 0, ext_lose, super::idl::s("lose"));
            op_with(vm, eo, "restoreContext", 0, ext_lose, super::idl::s("restore"));
        }
    }
    let tag = PropertyKey::Sym(vm.wk.to_string_tag.clone());
    vm.heap.get_mut(eo).props.insert(tag, Prop::data(super::idl::s(canon), C));
    vm.create_data_property(exts, PropertyKey::from_str(canon), Value::Object(eo))?;
    Ok(Value::Object(eo))
}

/// WEBGL_lose_context.loseContext / restoreContext: the lose / restore steps, their events queued as tasks
/// (WebGL 1.0 "lose the context", "restore the context").
fn ext_lose(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(e) = this_tagged(vm, &ctx.this, T_GLEXT) else { return throw_illegal(vm) };
    let sid = slot_num(vm, e, 0) as u32;
    let ctxo = slot(vm, e, 1);
    let restore = callee_str(vm, ctx) == "restore";
    if restore {
        if !is_lost(sid) {
            gl(sid, |g| g.err(c::INVALID_OPERATION));
            return Ok(Value::Undefined);
        }
        if !STATE.with(|s| s.borrow().restorable.get(&sid).copied().unwrap_or(false)) {
            return Ok(Value::Undefined);
        }
    } else {
        if is_lost(sid) {
            return Ok(Value::Undefined);
        }
        let exts = canvas::with_gl(sid, |g| g.ext);
        gl(sid, |g| {
            g.lose();
            if let Some(x) = exts {
                g.ext = x;
            }
        });
        STATE.with(|s| s.borrow_mut().restorable.insert(sid, false));
    }
    let fp = vm.intr().function_proto;
    let f = vm.make_native_with("", 0, fire_context_event, false, Some(fp), vec![ctxo, Value::Bool(restore)]);
    let g = vm.realm().global;
    let st = get(vm, g, "setTimeout")?;
    vm.call(&st, &Value::Undefined, &[Value::Object(f), num(0.0)])?;
    Ok(Value::Undefined)
}

fn fire_context_event(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let ctxo = vm.native_slot(ctx.callee, 0);
    let restore = boolean(vm, &vm.native_slot(ctx.callee, 1));
    let Some(o) = ctxo.as_object() else { return Ok(Value::Undefined) };
    let sid = slot_num(vm, o, 0) as u32;
    let canvas_v = slot(vm, o, 1);
    let Some(el) = dom::node_of(vm, &canvas_v) else { return Ok(Value::Undefined) };
    if restore {
        let exts = canvas::with_gl(sid, |g| g.ext);
        gl(sid, |g| {
            g.restore();
            if let Some(x) = exts {
                g.ext = x;
            }
        });
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            s.restorable.insert(sid, false);
            s.objs.retain(|k, _| k.0 != sid);
        });
    }
    let init = vm.new_plain_object();
    vm.create_data_property(init, PropertyKey::from_str("statusMessage"), super::idl::s(""))?;
    let ty = if restore { "webglcontextrestored" } else { "webglcontextlost" };
    let e = super::events::new_event(vm, "WebGLContextEvent", ty, false, !restore, Some(init));
    let r = root(vm, Value::Object(e));
    let (not_canceled, _) = super::events::dispatch(vm, e, TargetKey::Node(el));
    unroot(vm, r);
    if !restore {
        STATE.with(|s| s.borrow_mut().restorable.insert(sid, !not_canceled));
    }
    Ok(Value::Undefined)
}

fn ctx_event_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'WebGLContextEvent': Please use the 'new' operator");
    }
    need(vm, ctx, 1, "WebGLContextEvent", "constructor")?;
    let ty = string(vm, &arg(vm, ctx, 0))?;
    let dict = arg(vm, ctx, 1);
    let b = dict_member(vm, &dict, "bubbles")?;
    let cn = dict_member(vm, &dict, "cancelable")?;
    let comp = dict_member(vm, &dict, "composed")?;
    let msg = dict_member(vm, &dict, "statusMessage")?;
    let msg = if msg.is_undefined() { String::new() } else { string(vm, &msg)? };
    let init = vm.new_plain_object();
    vm.create_data_property(init, PropertyKey::from_str("statusMessage"), super::idl::s(&msg))?;
    let (bb, cc) = (boolean(vm, &b), boolean(vm, &cn));
    let e = super::events::new_event(vm, "WebGLContextEvent", &ty, bb, cc, Some(init));
    // A constructed event is untrusted (DOM §2.5); composed comes from the dictionary.
    let mut f = slot_num(vm, e, 1) as u32 & !F_TRUSTED;
    if boolean(vm, &comp) {
        f |= F_COMPOSED;
    }
    set_slot(vm, e, 1, num(f as f64));
    let default = iface("WebGLContextEvent").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    vm.heap.get_mut(e).proto = Some(proto);
    Ok(Value::Object(e))
}

fn ctx_event_status(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = ctx.this.as_object() else { return throw_illegal(vm) };
    let init = slot(vm, o, 6);
    match init.as_object() {
        Some(i) => get(vm, i, "statusMessage"),
        None => Ok(super::idl::s("")),
    }
}

// ------------------------------------------------------------------------------------------------ attributes

fn gl_attr(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, sid) = this_gl(vm, ctx)?;
    match callee_str(vm, ctx).as_str() {
        "canvas" => Ok(slot(vm, o, 1)),
        "drawingBufferWidth" => Ok(num(gl(sid, |g| g.db.w) as f64)),
        "drawingBufferHeight" => Ok(num(gl(sid, |g| g.db.h) as f64)),
        _ => Ok(super::idl::s("srgb")),
    }
}

// ------------------------------------------------------------------------------------------------ methods

/// (name, required argument count) of every WebGLRenderingContext operation of the WebGL 1.0 IDL.
const METHODS: &[(&str, u32)] = &[
    ("activeTexture", 1),
    ("attachShader", 2),
    ("bindAttribLocation", 3),
    ("bindBuffer", 2),
    ("bindFramebuffer", 2),
    ("bindRenderbuffer", 2),
    ("bindTexture", 2),
    ("blendColor", 4),
    ("blendEquation", 1),
    ("blendEquationSeparate", 2),
    ("blendFunc", 2),
    ("blendFuncSeparate", 4),
    ("bufferData", 3),
    ("bufferSubData", 3),
    ("checkFramebufferStatus", 1),
    ("clear", 1),
    ("clearColor", 4),
    ("clearDepth", 1),
    ("clearStencil", 1),
    ("colorMask", 4),
    ("compileShader", 1),
    ("compressedTexImage2D", 7),
    ("compressedTexSubImage2D", 8),
    ("copyTexImage2D", 8),
    ("copyTexSubImage2D", 8),
    ("createBuffer", 0),
    ("createFramebuffer", 0),
    ("createProgram", 0),
    ("createRenderbuffer", 0),
    ("createShader", 1),
    ("createTexture", 0),
    ("cullFace", 1),
    ("deleteBuffer", 1),
    ("deleteFramebuffer", 1),
    ("deleteProgram", 1),
    ("deleteRenderbuffer", 1),
    ("deleteShader", 1),
    ("deleteTexture", 1),
    ("depthFunc", 1),
    ("depthMask", 1),
    ("depthRange", 2),
    ("detachShader", 2),
    ("disable", 1),
    ("disableVertexAttribArray", 1),
    ("drawArrays", 3),
    ("drawElements", 4),
    ("enable", 1),
    ("enableVertexAttribArray", 1),
    ("finish", 0),
    ("flush", 0),
    ("framebufferRenderbuffer", 4),
    ("framebufferTexture2D", 5),
    ("frontFace", 1),
    ("generateMipmap", 1),
    ("getActiveAttrib", 2),
    ("getActiveUniform", 2),
    ("getAttachedShaders", 1),
    ("getAttribLocation", 2),
    ("getBufferParameter", 2),
    ("getContextAttributes", 0),
    ("getError", 0),
    ("getExtension", 1),
    ("getFramebufferAttachmentParameter", 3),
    ("getParameter", 1),
    ("getProgramInfoLog", 1),
    ("getProgramParameter", 2),
    ("getRenderbufferParameter", 2),
    ("getShaderInfoLog", 1),
    ("getShaderParameter", 2),
    ("getShaderPrecisionFormat", 2),
    ("getShaderSource", 1),
    ("getSupportedExtensions", 0),
    ("getTexParameter", 2),
    ("getUniform", 2),
    ("getUniformLocation", 2),
    ("getVertexAttrib", 2),
    ("getVertexAttribOffset", 2),
    ("hint", 2),
    ("isBuffer", 1),
    ("isContextLost", 0),
    ("isEnabled", 1),
    ("isFramebuffer", 1),
    ("isProgram", 1),
    ("isRenderbuffer", 1),
    ("isShader", 1),
    ("isTexture", 1),
    ("lineWidth", 1),
    ("linkProgram", 1),
    ("pixelStorei", 2),
    ("polygonOffset", 2),
    ("readPixels", 7),
    ("renderbufferStorage", 4),
    ("sampleCoverage", 2),
    ("scissor", 4),
    ("shaderSource", 2),
    ("stencilFunc", 3),
    ("stencilFuncSeparate", 4),
    ("stencilMask", 1),
    ("stencilMaskSeparate", 2),
    ("stencilOp", 3),
    ("stencilOpSeparate", 4),
    ("texImage2D", 6),
    ("texParameterf", 3),
    ("texParameteri", 3),
    ("texSubImage2D", 7),
    ("uniform1f", 2),
    ("uniform1fv", 2),
    ("uniform1i", 2),
    ("uniform1iv", 2),
    ("uniform2f", 3),
    ("uniform2fv", 2),
    ("uniform2i", 3),
    ("uniform2iv", 2),
    ("uniform3f", 4),
    ("uniform3fv", 2),
    ("uniform3i", 4),
    ("uniform3iv", 2),
    ("uniform4f", 5),
    ("uniform4fv", 2),
    ("uniform4i", 5),
    ("uniform4iv", 2),
    ("uniformMatrix2fv", 3),
    ("uniformMatrix3fv", 3),
    ("uniformMatrix4fv", 3),
    ("useProgram", 1),
    ("validateProgram", 1),
    ("vertexAttrib1f", 2),
    ("vertexAttrib1fv", 2),
    ("vertexAttrib2f", 3),
    ("vertexAttrib2fv", 2),
    ("vertexAttrib3f", 4),
    ("vertexAttrib3fv", 2),
    ("vertexAttrib4f", 5),
    ("vertexAttrib4fv", 2),
    ("vertexAttribPointer", 6),
    ("viewport", 4),
];

/// What a method answers while the context is lost (WebGL 1.0 "lose the context": null / false / -1 / 0 /
/// FRAMEBUFFER_UNSUPPORTED, everything else undefined).
fn lost_default(name: &str) -> Value {
    match name {
        "isBuffer" | "isEnabled" | "isFramebuffer" | "isProgram" | "isRenderbuffer" | "isShader" | "isTexture" => Value::Bool(false),
        "getAttribLocation" => num(-1.0),
        "getVertexAttribOffset" => num(0.0),
        "checkFramebufferStatus" => num(c::FRAMEBUFFER_UNSUPPORTED as f64),
        n if n.starts_with("get") || n.starts_with("create") => Value::Null,
        _ => Value::Undefined,
    }
}

fn gl_method(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let (o, sid) = this_gl(vm, ctx)?;
    let name = callee_str(vm, ctx);
    let argc = METHODS.iter().find(|(n, _)| *n == name).map(|x| x.1).unwrap_or(0);
    need(vm, ctx, argc as usize, "WebGLRenderingContext", &name)?;
    match name.as_str() {
        "isContextLost" => return Ok(Value::Bool(is_lost(sid))),
        "getError" => return Ok(num(gl(sid, |g| g.get_error()) as f64)),
        _ => {}
    }
    if is_lost(sid) {
        return Ok(lost_default(&name));
    }
    let n = name.as_str();
    let u = Value::Undefined;
    macro_rules! obj {
        ($i:expr, $k:expr, $nullable:expr) => {
            match obj_arg(vm, ctx, $i, sid, $k, $nullable, n)? {
                Some(x) => x,
                None => return Ok(lost_default_foreign(n)),
            }
        };
    }
    Ok(match n {
        "getContextAttributes" => return context_attributes(vm, sid),
        "getSupportedExtensions" => Value::Object({
            let v: Vec<Value> = EXTENSIONS.iter().map(|e| super::idl::s(e)).collect();
            vm.new_array(v)
        }),
        "getExtension" => {
            let nm = string(vm, &arg(vm, ctx, 0))?;
            return get_extension(vm, o, sid, &nm);
        }
        "activeTexture" => {
            let t = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.active_texture(t));
            u
        }
        "attachShader" | "detachShader" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            let s = obj!(1, ObjKind::Shader, false).unwrap_or(0);
            if n == "attachShader" { gl(sid, |g| g.attach_shader(p, s)) } else { gl(sid, |g| g.detach_shader(p, s)) }
            u
        }
        "bindAttribLocation" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            let i = a_u32(vm, ctx, 1)?;
            let nm = string(vm, &arg(vm, ctx, 2))?;
            gl(sid, |g| g.bind_attrib_location(p, i, &nm));
            u
        }
        "bindBuffer" | "bindFramebuffer" | "bindRenderbuffer" | "bindTexture" => {
            let t = a_u32(vm, ctx, 0)?;
            let k = match n {
                "bindBuffer" => ObjKind::Buffer,
                "bindFramebuffer" => ObjKind::Framebuffer,
                "bindRenderbuffer" => ObjKind::Renderbuffer,
                _ => ObjKind::Texture,
            };
            let x = obj!(1, k, true).unwrap_or(0);
            gl(sid, |g| match k {
                ObjKind::Buffer => g.bind_buffer(t, x),
                ObjKind::Framebuffer => g.bind_framebuffer(t, x),
                ObjKind::Renderbuffer => g.bind_renderbuffer(t, x),
                _ => g.bind_texture(t, x),
            });
            u
        }
        "blendColor" | "clearColor" => {
            let v = [a_f32(vm, ctx, 0)?, a_f32(vm, ctx, 1)?, a_f32(vm, ctx, 2)?, a_f32(vm, ctx, 3)?];
            if n == "blendColor" { gl(sid, |g| g.blend_color(v)) } else { gl(sid, |g| g.clear_color(v)) }
            u
        }
        "blendEquation" => {
            let m = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.blend_equation_separate(m, m));
            u
        }
        "blendEquationSeparate" => {
            let (a, b) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?);
            gl(sid, |g| g.blend_equation_separate(a, b));
            u
        }
        "blendFunc" => {
            let (s, d) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?);
            gl(sid, |g| g.blend_func_separate(s, d, s, d));
            u
        }
        "blendFuncSeparate" => {
            let v = [a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?, a_u32(vm, ctx, 3)?];
            gl(sid, |g| g.blend_func_separate(v[0], v[1], v[2], v[3]));
            u
        }
        "bufferData" => {
            let t = a_u32(vm, ctx, 0)?;
            let d = arg(vm, ctx, 1);
            let usage = a_u32(vm, ctx, 2)?;
            if d.is_null() {
                gl(sid, |g| g.err(c::INVALID_VALUE));
            } else if d.is_object() {
                let Some(bytes) = super::net::buffer_bytes(vm, &d) else {
                    return vm.throw_type("Failed to execute 'bufferData' on 'WebGLRenderingContext': parameter 2 is not of type 'ArrayBuffer'.");
                };
                gl(sid, |g| g.buffer_data(t, bytes.len() as i64, Some(&bytes), usage));
            } else {
                let size = a_i64(vm, ctx, 1)?;
                gl(sid, |g| g.buffer_data(t, size, None, usage));
            }
            u
        }
        "bufferSubData" => {
            let t = a_u32(vm, ctx, 0)?;
            let off = a_i64(vm, ctx, 1)?;
            let d = arg(vm, ctx, 2);
            let Some(bytes) = super::net::buffer_bytes(vm, &d) else {
                return vm.throw_type("Failed to execute 'bufferSubData' on 'WebGLRenderingContext': parameter 3 is not of type 'ArrayBuffer'.");
            };
            gl(sid, |g| g.buffer_sub_data(t, off, &bytes));
            u
        }
        "checkFramebufferStatus" => {
            let t = a_u32(vm, ctx, 0)?;
            num(gl(sid, |g| g.check_framebuffer_status(t)) as f64)
        }
        "clear" => {
            let m = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.clear(m));
            u
        }
        "clearDepth" => {
            let d = a_f32(vm, ctx, 0)?;
            gl(sid, |g| g.clear_depth(d));
            u
        }
        "clearStencil" => {
            let s = a_i32(vm, ctx, 0)?;
            gl(sid, |g| g.clear_stencil(s));
            u
        }
        "colorMask" => {
            let m = [a_bool(vm, ctx, 0), a_bool(vm, ctx, 1), a_bool(vm, ctx, 2), a_bool(vm, ctx, 3)];
            gl(sid, |g| g.color_mask(m));
            u
        }
        "compileShader" | "deleteShader" => {
            let s = obj!(0, ObjKind::Shader, n == "deleteShader");
            if let Some(s) = s {
                if n == "compileShader" { gl(sid, |g| g.compile_shader(s)) } else { gl(sid, |g| g.delete_shader(s)) }
            }
            u
        }
        "compressedTexImage2D" | "compressedTexSubImage2D" => {
            gl(sid, |g| g.compressed_tex_image());
            u
        }
        "copyTexImage2D" => {
            let t = a_u32(vm, ctx, 0)?;
            let v = [a_i32(vm, ctx, 1)?, a_i32(vm, ctx, 3)?, a_i32(vm, ctx, 4)?, a_i32(vm, ctx, 5)?, a_i32(vm, ctx, 6)?, a_i32(vm, ctx, 7)?];
            let f = a_u32(vm, ctx, 2)?;
            gl(sid, |g| g.copy_tex_image_2d(t, v[0], f, v[1], v[2], v[3], v[4], v[5]));
            u
        }
        "copyTexSubImage2D" => {
            let t = a_u32(vm, ctx, 0)?;
            let mut v = [0i32; 7];
            for (k, x) in v.iter_mut().enumerate() {
                *x = a_i32(vm, ctx, k + 1)?;
            }
            gl(sid, |g| g.copy_tex_sub_image_2d(t, v[0], v[1], v[2], v[3], v[4], v[5], v[6]));
            u
        }
        "createBuffer" => {
            let x = gl(sid, |g| g.create_buffer());
            wrap_obj(vm, sid, ObjKind::Buffer, x)
        }
        "createFramebuffer" => {
            let x = gl(sid, |g| g.create_framebuffer());
            wrap_obj(vm, sid, ObjKind::Framebuffer, x)
        }
        "createProgram" => {
            let x = gl(sid, |g| g.create_program());
            wrap_obj(vm, sid, ObjKind::Program, x)
        }
        "createRenderbuffer" => {
            let x = gl(sid, |g| g.create_renderbuffer());
            wrap_obj(vm, sid, ObjKind::Renderbuffer, x)
        }
        "createTexture" => {
            let x = gl(sid, |g| g.create_texture());
            wrap_obj(vm, sid, ObjKind::Texture, x)
        }
        "createShader" => {
            let t = a_u32(vm, ctx, 0)?;
            let x = gl(sid, |g| g.create_shader(t));
            wrap_obj(vm, sid, ObjKind::Shader, x)
        }
        "cullFace" => {
            let m = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.cull_face(m));
            u
        }
        "deleteBuffer" | "deleteFramebuffer" | "deleteProgram" | "deleteRenderbuffer" | "deleteTexture" => {
            let k = match n {
                "deleteBuffer" => ObjKind::Buffer,
                "deleteFramebuffer" => ObjKind::Framebuffer,
                "deleteProgram" => ObjKind::Program,
                "deleteRenderbuffer" => ObjKind::Renderbuffer,
                _ => ObjKind::Texture,
            };
            if let Some(x) = obj!(0, k, true) {
                gl(sid, |g| match k {
                    ObjKind::Buffer => g.delete_buffer(x),
                    ObjKind::Framebuffer => g.delete_framebuffer(x),
                    ObjKind::Program => g.delete_program(x),
                    ObjKind::Renderbuffer => g.delete_renderbuffer(x),
                    _ => g.delete_texture(x),
                });
            }
            u
        }
        "depthFunc" => {
            let f = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.depth_func(f));
            u
        }
        "depthMask" => {
            let m = a_bool(vm, ctx, 0);
            gl(sid, |g| g.depth_mask(m));
            u
        }
        "depthRange" => {
            let (a, b) = (a_f32(vm, ctx, 0)?, a_f32(vm, ctx, 1)?);
            gl(sid, |g| g.depth_range(a, b));
            u
        }
        "disable" | "enable" => {
            let cap = a_u32(vm, ctx, 0)?;
            let on = n == "enable";
            gl(sid, |g| g.enable(cap, on));
            u
        }
        "disableVertexAttribArray" | "enableVertexAttribArray" => {
            let i = a_u32(vm, ctx, 0)?;
            let on = n == "enableVertexAttribArray";
            gl(sid, |g| g.enable_vertex_attrib_array(i, on));
            u
        }
        "drawArrays" => {
            let (m, f, cnt) = (a_u32(vm, ctx, 0)?, a_i32(vm, ctx, 1)?, a_i32(vm, ctx, 2)?);
            gl(sid, |g| g.draw_arrays(m, f, cnt));
            u
        }
        "drawElements" => {
            let (m, cnt, t, off) = (a_u32(vm, ctx, 0)?, a_i32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?, a_i64(vm, ctx, 3)?);
            gl(sid, |g| g.draw_elements(m, cnt, t, off));
            u
        }
        "finish" | "flush" => u,
        "framebufferRenderbuffer" => {
            let (t, a, rt) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?);
            let r = obj!(3, ObjKind::Renderbuffer, true).unwrap_or(0);
            gl(sid, |g| g.framebuffer_renderbuffer(t, a, rt, r));
            u
        }
        "framebufferTexture2D" => {
            let (t, a, tt) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?);
            let x = obj!(3, ObjKind::Texture, true).unwrap_or(0);
            let l = a_i32(vm, ctx, 4)?;
            gl(sid, |g| g.framebuffer_texture_2d(t, a, tt, x, l));
            u
        }
        "frontFace" => {
            let m = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.front_face(m));
            u
        }
        "generateMipmap" => {
            let t = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.generate_mipmap(t));
            u
        }
        "getActiveAttrib" | "getActiveUniform" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            let i = a_u32(vm, ctx, 1)?;
            let r = if n == "getActiveAttrib" { canvas::with_gl(sid, |g| g.get_active_attrib(p, i)) } else { canvas::with_gl(sid, |g| g.get_active_uniform(p, i)) };
            match r.flatten() {
                Some(info) => info_obj(vm, info),
                None => Value::Null,
            }
        }
        "getAttachedShaders" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            match canvas::with_gl(sid, |g| g.get_attached_shaders(p)).flatten() {
                Some(list) => {
                    let v: Vec<Value> = list.into_iter().map(|s| wrap_obj(vm, sid, ObjKind::Shader, s)).collect();
                    Value::Object(vm.new_array(v))
                }
                None => Value::Null,
            }
        }
        "getAttribLocation" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            let nm = string(vm, &arg(vm, ctx, 1))?;
            num(gl(sid, |g| g.get_attrib_location(p, &nm)) as f64)
        }
        "getBufferParameter" | "getRenderbufferParameter" | "getTexParameter" | "getParameter" | "getVertexAttrib" => {
            let a = a_u32(vm, ctx, 0)?;
            let b = if n == "getParameter" { 0 } else { a_u32(vm, ctx, 1)? };
            let p = canvas::with_gl(sid, |g| match n {
                "getBufferParameter" => g.get_buffer_parameter(a, b),
                "getRenderbufferParameter" => g.get_renderbuffer_parameter(a, b),
                "getTexParameter" => g.get_tex_parameter(a, b),
                "getVertexAttrib" => g.get_vertex_attrib(a, b),
                _ => g.get_parameter(a),
            })
            .unwrap_or(Param::Null);
            param_value(vm, sid, p)?
        }
        "getFramebufferAttachmentParameter" => {
            let (t, a, p) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?);
            let r = canvas::with_gl(sid, |g| g.get_framebuffer_attachment_parameter(t, a, p)).unwrap_or(Param::Null);
            param_value(vm, sid, r)?
        }
        "getProgramInfoLog" | "getShaderInfoLog" | "getShaderSource" => {
            let r = if n == "getProgramInfoLog" {
                let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
                canvas::with_gl(sid, |g| g.get_program_info_log(p))
            } else {
                let s = obj!(0, ObjKind::Shader, false).unwrap_or(0);
                if n == "getShaderSource" { canvas::with_gl(sid, |g| g.get_shader_source(s)) } else { canvas::with_gl(sid, |g| g.get_shader_info_log(s)) }
            };
            match r.flatten() {
                Some(t) => super::idl::s(&t),
                None => Value::Null,
            }
        }
        "getProgramParameter" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            let pn = a_u32(vm, ctx, 1)?;
            let r = canvas::with_gl(sid, |g| g.get_program_parameter(p, pn)).unwrap_or(Param::Null);
            param_value(vm, sid, r)?
        }
        "getShaderParameter" => {
            let s = obj!(0, ObjKind::Shader, false).unwrap_or(0);
            let pn = a_u32(vm, ctx, 1)?;
            let r = canvas::with_gl(sid, |g| g.get_shader_parameter(s, pn)).unwrap_or(Param::Null);
            param_value(vm, sid, r)?
        }
        "getShaderPrecisionFormat" => {
            let (a, b) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?);
            match canvas::with_gl(sid, |g| g.get_shader_precision_format(a, b)).flatten() {
                Some(f) => {
                    let proto = iface("WebGLShaderPrecisionFormat").map(|x| x.proto).unwrap_or_else(|| vm.intr().object_proto);
                    Value::Object(host_obj(vm, proto, T_GLPREC, vec![num(f.range_min as f64), num(f.range_max as f64), num(f.precision as f64)]))
                }
                None => Value::Null,
            }
        }
        "getUniform" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            let Some(Some(l)) = loc_arg(vm, ctx, 1, sid, n)? else {
                if arg(vm, ctx, 1).is_nullish() {
                    return vm.throw_type("Failed to execute 'getUniform' on 'WebGLRenderingContext': parameter 2 is not of type 'WebGLUniformLocation'.");
                }
                return Ok(Value::Null);
            };
            let r = canvas::with_gl(sid, |g| g.get_uniform(p, &l)).unwrap_or(Param::Null);
            param_value(vm, sid, r)?
        }
        "getUniformLocation" => {
            let p = obj!(0, ObjKind::Program, false).unwrap_or(0);
            let nm = string(vm, &arg(vm, ctx, 1))?;
            match canvas::with_gl(sid, |g| g.get_uniform_location(p, &nm)).flatten() {
                Some(l) => {
                    let proto = iface("WebGLUniformLocation").map(|x| x.proto).unwrap_or_else(|| vm.intr().object_proto);
                    Value::Object(host_obj(vm, proto, T_GLLOC, vec![num(sid as f64), num(l.program as f64), num(l.link_gen as f64), num(l.index as f64)]))
                }
                None => Value::Null,
            }
        }
        "getVertexAttribOffset" => {
            let (i, p) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?);
            num(gl(sid, |g| g.get_vertex_attrib_offset(i, p)) as f64)
        }
        "hint" => {
            let (t, m) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?);
            gl(sid, |g| g.hint(t, m));
            u
        }
        "isBuffer" | "isFramebuffer" | "isProgram" | "isRenderbuffer" | "isShader" | "isTexture" => {
            let k = match n {
                "isBuffer" => ObjKind::Buffer,
                "isFramebuffer" => ObjKind::Framebuffer,
                "isProgram" => ObjKind::Program,
                "isRenderbuffer" => ObjKind::Renderbuffer,
                "isShader" => ObjKind::Shader,
                _ => ObjKind::Texture,
            };
            let v = arg(vm, ctx, 0);
            let mine = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_GLOBJ) && slot_num(vm, *o, 1) as u8 == kind_index(k) && slot_num(vm, *o, 0) as u32 == sid);
            match mine {
                Some(x) => {
                    let nm = slot_num(vm, x, 2) as u32;
                    Value::Bool(gl(sid, |g| g.is_object(k, nm)))
                }
                None => {
                    if !v.is_nullish() && !v.as_object().is_some_and(|o| tag_of(vm, o) == Some(T_GLOBJ)) {
                        return vm.throw_type(&format!("Failed to execute '{n}' on 'WebGLRenderingContext': parameter 1 is not of type '{}'.", OBJ_IFACES[kind_index(k) as usize]));
                    }
                    Value::Bool(false)
                }
            }
        }
        "isEnabled" => {
            let cap = a_u32(vm, ctx, 0)?;
            Value::Bool(gl(sid, |g| g.is_enabled(cap)))
        }
        "lineWidth" => {
            let w = a_f32(vm, ctx, 0)?;
            gl(sid, |g| g.line_width(w));
            u
        }
        "linkProgram" | "useProgram" | "validateProgram" => {
            let p = obj!(0, ObjKind::Program, n == "useProgram");
            match (n, p) {
                ("linkProgram", Some(p)) => gl(sid, |g| g.link_program(p)),
                ("validateProgram", Some(p)) => gl(sid, |g| g.validate_program(p)),
                ("useProgram", p) => gl(sid, |g| g.use_program(p.unwrap_or(0))),
                _ => {}
            }
            u
        }
        "pixelStorei" => {
            let (p, v) = (a_u32(vm, ctx, 0)?, a_i32(vm, ctx, 1)?);
            gl(sid, |g| g.pixel_storei(p, v));
            u
        }
        "polygonOffset" => {
            let (a, b) = (a_f32(vm, ctx, 0)?, a_f32(vm, ctx, 1)?);
            gl(sid, |g| g.polygon_offset(a, b));
            u
        }
        "readPixels" => {
            let v = [a_i32(vm, ctx, 0)?, a_i32(vm, ctx, 1)?, a_i32(vm, ctx, 2)?, a_i32(vm, ctx, 3)?];
            let (f, t) = (a_u32(vm, ctx, 4)?, a_u32(vm, ctx, 5)?);
            let dst = arg(vm, ctx, 6);
            if dst.is_null() {
                gl(sid, |g| g.err(c::INVALID_VALUE));
                return Ok(u);
            }
            let Some(ob) = dst.as_object() else {
                return vm.throw_type("Failed to execute 'readPixels' on 'WebGLRenderingContext': parameter 7 is not of type 'ArrayBufferView'.");
            };
            let rec = match &vm.heap.get(ob).kind {
                Kind::TypedArray(ta) => Some((ta.kind, ta.buffer, ta.byte_offset)),
                _ => None,
            };
            let Some((k, buf, off)) = rec else {
                return vm.throw_type("Failed to execute 'readPixels' on 'WebGLRenderingContext': parameter 7 is not of type 'ArrayBufferView'.");
            };
            if t == c::UNSIGNED_BYTE && !matches!(k, TAKind::Uint8 | TAKind::Uint8Clamped) {
                gl(sid, |g| g.err(c::INVALID_OPERATION));
                return Ok(u);
            }
            let len = ta_length(vm, ob).unwrap_or(0) * k.size();
            let mut bytes = match &vm.heap.get(buf).kind {
                Kind::ArrayBuffer(d) if !d.detached && off + len <= d.data.len() => d.data[off..off + len].to_vec(),
                _ => Vec::new(),
            };
            gl(sid, |g| g.read_pixels(v[0], v[1], v[2], v[3], f, t, &mut bytes));
            if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(buf).kind {
                if !d.detached && off + bytes.len() <= d.data.len() {
                    d.data[off..off + bytes.len()].copy_from_slice(&bytes);
                }
            }
            u
        }
        "renderbufferStorage" => {
            let (t, f, w, h) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_i32(vm, ctx, 2)?, a_i32(vm, ctx, 3)?);
            gl(sid, |g| g.renderbuffer_storage(t, f, w, h));
            u
        }
        "sampleCoverage" => {
            let (v, i) = (a_f32(vm, ctx, 0)?, a_bool(vm, ctx, 1));
            gl(sid, |g| g.sample_coverage(v, i));
            u
        }
        "scissor" | "viewport" => {
            let v = [a_i32(vm, ctx, 0)?, a_i32(vm, ctx, 1)?, a_i32(vm, ctx, 2)?, a_i32(vm, ctx, 3)?];
            if n == "scissor" { gl(sid, |g| g.scissor(v[0], v[1], v[2], v[3])) } else { gl(sid, |g| g.viewport(v[0], v[1], v[2], v[3])) }
            u
        }
        "shaderSource" => {
            let s = obj!(0, ObjKind::Shader, false).unwrap_or(0);
            let src = string(vm, &arg(vm, ctx, 1))?;
            gl(sid, |g| g.shader_source(s, &src));
            u
        }
        "stencilFunc" => {
            let (f, r, m) = (a_u32(vm, ctx, 0)?, a_i32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?);
            gl(sid, |g| g.stencil_func_separate(c::FRONT_AND_BACK, f, r, m));
            u
        }
        "stencilFuncSeparate" => {
            let (fc, f, r, m) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_i32(vm, ctx, 2)?, a_u32(vm, ctx, 3)?);
            gl(sid, |g| g.stencil_func_separate(fc, f, r, m));
            u
        }
        "stencilMask" => {
            let m = a_u32(vm, ctx, 0)?;
            gl(sid, |g| g.stencil_mask_separate(c::FRONT_AND_BACK, m));
            u
        }
        "stencilMaskSeparate" => {
            let (fc, m) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?);
            gl(sid, |g| g.stencil_mask_separate(fc, m));
            u
        }
        "stencilOp" => {
            let (a, b, d) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?);
            gl(sid, |g| g.stencil_op_separate(c::FRONT_AND_BACK, a, b, d));
            u
        }
        "stencilOpSeparate" => {
            let (fc, a, b, d) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?, a_u32(vm, ctx, 3)?);
            gl(sid, |g| g.stencil_op_separate(fc, a, b, d));
            u
        }
        "texImage2D" => {
            let t = a_u32(vm, ctx, 0)?;
            let level = a_i32(vm, ctx, 1)?;
            let internal = a_u32(vm, ctx, 2)?;
            if ctx.argc >= 9 {
                let (w, h, border) = (a_i32(vm, ctx, 3)?, a_i32(vm, ctx, 4)?, a_i32(vm, ctx, 5)?);
                let (f, ty) = (a_u32(vm, ctx, 6)?, a_u32(vm, ctx, 7)?);
                let pv = arg(vm, ctx, 8);
                let Some(px) = pixels_view(vm, &pv, ty, sid, n)? else { return Ok(u) };
                let pixels = match &px {
                    Some(b) => Pixels::Bytes(b),
                    None => Pixels::None,
                };
                gl(sid, |g| g.tex_image_2d(t, level, internal, w, h, border, f, ty, pixels));
            } else if ctx.argc == 6 {
                let (f, ty) = (a_u32(vm, ctx, 3)?, a_u32(vm, ctx, 4)?);
                let src = arg(vm, ctx, 5);
                if let Some((w, h, rgba)) = image_source(vm, &src)? {
                    gl(sid, |g| g.tex_image_2d(t, level, internal, w as i32, h as i32, 0, f, ty, Pixels::Rgba(&rgba)));
                }
            } else {
                return vm.throw_type("Failed to execute 'texImage2D' on 'WebGLRenderingContext': Valid arities are: [6, 9], but 7 arguments provided.");
            }
            u
        }
        "texSubImage2D" => {
            let t = a_u32(vm, ctx, 0)?;
            let level = a_i32(vm, ctx, 1)?;
            let (x, y) = (a_i32(vm, ctx, 2)?, a_i32(vm, ctx, 3)?);
            if ctx.argc >= 9 {
                let (w, h) = (a_i32(vm, ctx, 4)?, a_i32(vm, ctx, 5)?);
                let (f, ty) = (a_u32(vm, ctx, 6)?, a_u32(vm, ctx, 7)?);
                let pv = arg(vm, ctx, 8);
                let Some(px) = pixels_view(vm, &pv, ty, sid, n)? else { return Ok(u) };
                match &px {
                    Some(b) => gl(sid, |g| g.tex_sub_image_2d(t, level, x, y, w, h, f, ty, Pixels::Bytes(b))),
                    None => gl(sid, |g| g.err(c::INVALID_VALUE)),
                }
            } else if ctx.argc == 7 {
                let (f, ty) = (a_u32(vm, ctx, 4)?, a_u32(vm, ctx, 5)?);
                let src = arg(vm, ctx, 6);
                if let Some((w, h, rgba)) = image_source(vm, &src)? {
                    gl(sid, |g| g.tex_sub_image_2d(t, level, x, y, w as i32, h as i32, f, ty, Pixels::Rgba(&rgba)));
                }
            } else {
                return vm.throw_type("Failed to execute 'texSubImage2D' on 'WebGLRenderingContext': Valid arities are: [7, 9].");
            }
            u
        }
        "texParameterf" | "texParameteri" => {
            let (t, p) = (a_u32(vm, ctx, 0)?, a_u32(vm, ctx, 1)?);
            let v = if n == "texParameterf" { a_f32(vm, ctx, 2)? as i32 } else { a_i32(vm, ctx, 2)? };
            gl(sid, |g| g.tex_parameter(t, p, v));
            u
        }
        _ if n.starts_with("uniformMatrix") => {
            let k = n.as_bytes()[13] - b'0';
            let Some(loc) = loc_arg(vm, ctx, 0, sid, n)? else { return Ok(u) };
            let tr = a_bool(vm, ctx, 1);
            let v = arg(vm, ctx, 2);
            let data: Vec<f32> = number_seq(vm, &v, "Float32Array")?.into_iter().map(|x| x as f32).collect();
            gl(sid, |g| g.uniform_matrix(loc.as_ref(), k, tr, &data));
            u
        }
        _ if n.starts_with("uniform") => {
            let b = n.as_bytes();
            let comps = b[7] - b'0';
            let kind = if b[8] == b'f' { UKind::Float } else { UKind::Int };
            let vform = n.ends_with('v');
            let Some(loc) = loc_arg(vm, ctx, 0, sid, n)? else { return Ok(u) };
            let data: Vec<f64> = if vform {
                let v = arg(vm, ctx, 1);
                number_seq(vm, &v, if kind == UKind::Float { "Float32Array" } else { "Int32Array" })?
            } else {
                let mut d = Vec::with_capacity(comps as usize);
                for i in 0..comps as usize {
                    d.push(if kind == UKind::Float { a_f32(vm, ctx, i + 1)? as f64 } else { a_i32(vm, ctx, i + 1)? as f64 });
                }
                d
            };
            let data: Vec<f64> = if kind == UKind::Float { data.into_iter().map(|x| x as f32 as f64).collect() } else { data.into_iter().map(|x| js_core::vm::ops::to_int32(x) as f64).collect() };
            gl(sid, |g| g.uniform(loc.as_ref(), kind, comps, &data, vform));
            u
        }
        _ if n.starts_with("vertexAttrib") && n != "vertexAttribPointer" => {
            let k = (n.as_bytes()[12] - b'0') as usize;
            let i = a_u32(vm, ctx, 0)?;
            let vals: Vec<f32> = if n.ends_with('v') {
                let v = arg(vm, ctx, 1);
                let d = number_seq(vm, &v, "Float32Array")?;
                if d.len() < k {
                    gl(sid, |g| g.err(c::INVALID_VALUE));
                    return Ok(u);
                }
                d[..k].iter().map(|x| *x as f32).collect()
            } else {
                let mut d = Vec::with_capacity(k);
                for j in 0..k {
                    d.push(a_f32(vm, ctx, j + 1)?);
                }
                d
            };
            gl(sid, |g| g.vertex_attrib(i, &vals));
            u
        }
        "vertexAttribPointer" => {
            let (i, s, t) = (a_u32(vm, ctx, 0)?, a_i32(vm, ctx, 1)?, a_u32(vm, ctx, 2)?);
            let nz = a_bool(vm, ctx, 3);
            let (st, off) = (a_i32(vm, ctx, 4)?, a_i64(vm, ctx, 5)?);
            gl(sid, |g| g.vertex_attrib_pointer(i, s, t, nz, st, off));
            u
        }
        _ => u,
    })
}

/// A call whose object argument came from another context answers like a lost call (null / false / -1).
fn lost_default_foreign(name: &str) -> Value {
    lost_default(name)
}

// ------------------------------------------------------------------------------------------------ install

pub fn install(vm: &mut Vm) {
    STATE.with(|s| *s.borrow_mut() = GlState::default());
    let ctxi = interface(vm, "WebGLRenderingContext", None, None);
    register_iface("WebGLRenderingContext", ctxi);
    for (n, v) in gl_core::consts::TABLE {
        konst(vm, ctxi, n, *v as f64);
    }
    for (n, len) in METHODS {
        op_with(vm, ctxi.proto, n, *len, gl_method, super::idl::s(n));
    }
    for n in ["canvas", "drawingBufferWidth", "drawingBufferHeight", "drawingBufferColorSpace"] {
        attr_with(vm, ctxi.proto, n, gl_attr, None, super::idl::s(n));
    }
    let obj = interface(vm, "WebGLObject", None, None);
    register_iface("WebGLObject", obj);
    for n in OBJ_IFACES {
        let i = interface(vm, n, Some(obj), None);
        register_iface(n, i);
    }
    let loc = interface(vm, "WebGLUniformLocation", None, None);
    register_iface("WebGLUniformLocation", loc);
    let info = interface(vm, "WebGLActiveInfo", None, None);
    register_iface("WebGLActiveInfo", info);
    for (i, n) in ["size", "type", "name"].iter().enumerate() {
        attr_with(vm, info.proto, n, info_get, None, super::idl::s(&i.to_string()));
    }
    let prec = interface(vm, "WebGLShaderPrecisionFormat", None, None);
    register_iface("WebGLShaderPrecisionFormat", prec);
    for (i, n) in ["rangeMin", "rangeMax", "precision"].iter().enumerate() {
        attr_with(vm, prec.proto, n, prec_get, None, super::idl::s(&i.to_string()));
    }
    let ev = iface("Event");
    let cev = interface(vm, "WebGLContextEvent", ev, Some((ctx_event_ctor, 1)));
    register_iface("WebGLContextEvent", cev);
    attr(vm, cev.proto, "statusMessage", ctx_event_status, None);
}
