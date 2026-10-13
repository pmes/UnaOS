//! AETHERCANVAS (LEDGER SR85): the WebIDL face of the 2D canvas (HTML §4.12.5) over `crate::canvas`.
//!
//! Interfaces: `CanvasRenderingContext2D`, `OffscreenCanvasRenderingContext2D`, `Path2D`, `CanvasGradient`,
//! `CanvasPattern`, `ImageData`, `TextMetrics`, `OffscreenCanvas`, `ImageBitmap`; on `HTMLCanvasElement`
//! `getContext` (`'2d'` with its settings dictionary; `webgl` / `experimental-webgl` are js/webgl.rs's (WEBGL1,
//! SR136); `webgl2`/`webgpu`/`bitmaprenderer` answer null), `toDataURL`, `toBlob`, and `width`/`height` whose setters reset the
//! bitmap and the context (also on a same-value set). A JS prelude adds `DOMMatrixReadOnly`/`DOMMatrix`
//! (what `getTransform` returns), `toBlob`, `OffscreenCanvas.convertToBlob` and `createImageBitmap`.
//!
//! Platform-object state: a context holds its surface id (the bitmap and the state stack live in
//! `crate::canvas`); a Path2D, gradient, pattern or ImageBitmap keeps its data in a hidden ArrayBuffer in
//! its own slots, so it lives and dies with the object (no Rust-side registry to leak).

use super::dom::{self, attr_value, iface, register_iface, set_attr, with_doc};
use super::idl::*;
use super::page;
use crate::canvas::{
    self,
    composite::Op,
    context::Style,
    geom::{parse_svg_path, Matrix, Path, Seg, P},
    paint::{Bitmap, Gradient, GradientKind, Paint, Repeat, Stop},
    raster::{Cap, Join},
    text::{self, Align, Baseline},
};
use html_core::NodeId;
use js_core::builtins::typedarray::{new_array_buffer, KINDS};
use js_core::vm::*;
use std::cell::Cell;
use std::rc::Rc;

const T_CTX: u8 = 60;
const T_PATH: u8 = 61;
const T_GRAD: u8 = 62;
const T_PAT: u8 = 63;
const T_IDATA: u8 = 64;
const T_METRICS: u8 = 65;
const T_OFF: u8 = 66;
const T_BMP: u8 = 67;

/// `[SameObject]` slot of an element's 2D context.
const SO_CTX: u8 = 200;

thread_local! {
    /// host root of the prelude's DOMMatrix constructor
    static DOMMATRIX: Cell<Option<usize>> = const { Cell::new(None) };
}

// ------------------------------------------------------------------------------------------------ helpers

fn num_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<f64> {
    let v = arg(vm, ctx, i);
    vm.to_number(&v)
}

fn nums(vm: &mut Vm, ctx: &CallCtx, k: usize) -> JsResult<Vec<f64>> {
    let mut out = Vec::with_capacity(k);
    for i in 0..k {
        out.push(num_arg(vm, ctx, i)?);
    }
    Ok(out)
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

fn non_finite<T>(vm: &mut Vm, iface_name: &str, method: &str) -> JsResult<T> {
    vm.throw_type(&format!("Failed to execute '{method}' on '{iface_name}': The provided double value is non-finite."))
}

/// HTML "rules for parsing non-negative integers".
fn parse_nonneg(s: &str) -> Option<u64> {
    let t = s.trim_start_matches([' ', '\t', '\n', '\x0c', '\r']);
    let t = t.strip_prefix('+').unwrap_or(t);
    let digits: String = t.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<u64>().ok().or(Some(u64::MAX))
}

fn dim_attr(el: usize, name: &str, default: u64) -> u64 {
    match attr_value(el, name).and_then(|v| parse_nonneg(&v)) {
        Some(v) if v <= 2_147_483_647 => v,
        _ => default,
    }
}

fn element_size(el: usize) -> (usize, usize) {
    (dim_attr(el, "width", 300) as usize, dim_attr(el, "height", 150) as usize)
}

fn is_canvas(el: usize) -> bool {
    with_doc(|d| dom::is_html_tag(d, el, "canvas"))
}

fn tag_is(el: usize, tag: &str) -> bool {
    with_doc(|d| dom::is_html_tag(d, el, tag))
}

fn new_hidden_buffer(vm: &mut Vm, bytes: Vec<u8>) -> JsResult<Obj> {
    let b = new_array_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(b).kind {
        d.data = bytes;
    }
    Ok(b)
}

fn hidden_bytes<'a>(vm: &'a Vm, v: &Value) -> Option<&'a Vec<u8>> {
    let o = v.as_object()?;
    match &vm.heap.get(o).kind {
        Kind::ArrayBuffer(d) => Some(&d.data),
        _ => None,
    }
}

fn hidden_bytes_mut<'a>(vm: &'a mut Vm, v: &Value) -> Option<&'a mut Vec<u8>> {
    let o = v.as_object()?;
    match &mut vm.heap.get_mut(o).kind {
        Kind::ArrayBuffer(d) => Some(&mut d.data),
        _ => None,
    }
}

fn f64s_to_bytes(v: &[f64]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 8);
    for x in v {
        out.extend_from_slice(&x.to_le_bytes());
    }
    out
}

fn bytes_to_f64s(b: &[u8]) -> Vec<f64> {
    b.chunks_exact(8).map(|c| f64::from_le_bytes([c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]])).collect()
}

/// A new `Uint8ClampedArray` over `bytes`.
fn clamped_array(vm: &mut Vm, bytes: Vec<u8>) -> JsResult<Obj> {
    let len = bytes.len();
    let b = new_array_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(b).kind {
        d.data = bytes;
    }
    let ki = KINDS.iter().position(|k| *k == TAKind::Uint8Clamped).unwrap_or(2);
    let proto = vm.intr().ta_protos[ki];
    Ok(vm.alloc(ObjectData::new(
        Some(proto),
        Kind::TypedArray(Box::new(TypedArrayData { kind: TAKind::Uint8Clamped, buffer: b, byte_offset: 0, length: Some(len) })),
    )))
}

fn is_clamped_array(vm: &Vm, v: &Value) -> bool {
    match v.as_object().map(|o| &vm.heap.get(o).kind) {
        Some(Kind::TypedArray(t)) => t.kind == TAKind::Uint8Clamped,
        _ => false,
    }
}

fn throw_illegal<T>(vm: &mut Vm) -> JsResult<T> {
    vm.throw_type("Illegal invocation")
}

// ------------------------------------------------------------------------------------------------ paths

fn encode_header(p: &Path) -> [f64; 5] {
    match p.cur {
        Some(c) => [1.0, c.0, c.1, p.start.0, p.start.1],
        None => [0.0, 0.0, 0.0, p.start.0, p.start.1],
    }
}

fn encode_segs(segs: &[Seg], out: &mut Vec<f64>) {
    for s in segs {
        match *s {
            Seg::Move(p) => out.extend_from_slice(&[1.0, p.0, p.1]),
            Seg::Line(p) => out.extend_from_slice(&[2.0, p.0, p.1]),
            Seg::Quad(c, p) => out.extend_from_slice(&[3.0, c.0, c.1, p.0, p.1]),
            Seg::Cubic(a, b, p) => out.extend_from_slice(&[4.0, a.0, a.1, b.0, b.1, p.0, p.1]),
            Seg::Close => out.push(5.0),
        }
    }
}

fn encode_path(p: &Path) -> Vec<u8> {
    let mut v: Vec<f64> = encode_header(p).to_vec();
    encode_segs(&p.segs, &mut v);
    f64s_to_bytes(&v)
}

fn decode_header(f: &[f64]) -> Path {
    let mut p = Path::new();
    if f.len() >= 5 {
        p.cur = (f[0] != 0.0).then_some((f[1], f[2]));
        p.start = (f[3], f[4]);
    }
    p
}

fn decode_path(b: &[u8]) -> Path {
    let f = bytes_to_f64s(b);
    let mut p = decode_header(&f);
    let mut i = 5;
    let pt = |i: usize| -> P { (f.get(i).copied().unwrap_or(0.0), f.get(i + 1).copied().unwrap_or(0.0)) };
    while i < f.len() {
        match f[i] as u8 {
            1 => {
                p.segs.push(Seg::Move(pt(i + 1)));
                i += 3;
            }
            2 => {
                p.segs.push(Seg::Line(pt(i + 1)));
                i += 3;
            }
            3 => {
                p.segs.push(Seg::Quad(pt(i + 1), pt(i + 3)));
                i += 5;
            }
            4 => {
                p.segs.push(Seg::Cubic(pt(i + 1), pt(i + 3), pt(i + 5)));
                i += 7;
            }
            _ => {
                p.segs.push(Seg::Close);
                i += 1;
            }
        }
    }
    p
}

fn path2d_of(vm: &Vm, v: &Value) -> Option<Path> {
    let o = v.as_object()?;
    if tag_of(vm, o) != Some(T_PATH) {
        return None;
    }
    hidden_bytes(vm, &slot(vm, o, 0)).map(|b| decode_path(b))
}

/// Run a path-building closure against `this` — the context's current default path (under the CTM) or a
/// Path2D (identity). Only the new segments are appended to a Path2D's buffer.
fn with_path<R>(vm: &mut Vm, ctx: &CallCtx, f: impl FnOnce(&mut Path, &Matrix) -> R) -> JsResult<R> {
    let Some(o) = ctx.this.as_object() else { return throw_illegal(vm) };
    match tag_of(vm, o) {
        Some(T_CTX) => {
            let id = surface_of_ctx(vm, o);
            match canvas::with(id, |c| {
                let m = c.state.ctm;
                f(&mut c.path, &m)
            }) {
                Some(r) => Ok(r),
                None => throw_illegal(vm),
            }
        }
        Some(T_PATH) => {
            let buf = slot(vm, o, 0);
            let head = hidden_bytes(vm, &buf).map(|b| bytes_to_f64s(&b[..b.len().min(40)])).unwrap_or_default();
            let mut p = decode_header(&head);
            let r = f(&mut p, &Matrix::IDENTITY);
            let mut add = Vec::new();
            encode_segs(&p.segs, &mut add);
            let header = f64s_to_bytes(&encode_header(&p));
            if let Some(data) = hidden_bytes_mut(vm, &buf) {
                if data.len() < 40 {
                    data.resize(40, 0);
                }
                data[..40].copy_from_slice(&header);
                data.extend_from_slice(&f64s_to_bytes(&add));
            }
            Ok(r)
        }
        _ => throw_illegal(vm),
    }
}

fn path_method(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let name = callee_str(vm, ctx);
    let iname = match ctx.this.as_object().and_then(|o| tag_of(vm, o)) {
        Some(T_PATH) => "Path2D",
        _ => "CanvasRenderingContext2D",
    };
    let (need_n, k) = match name.as_str() {
        "closePath" => (0, 0),
        "moveTo" | "lineTo" => (2, 2),
        "quadraticCurveTo" | "rect" => (4, 4),
        "bezierCurveTo" => (6, 6),
        "arcTo" => (5, 5),
        "arc" => (5, 5),
        "ellipse" => (7, 7),
        "roundRect" => (4, 4),
        _ => (0, 0),
    };
    need(vm, ctx, need_n, iname, &name)?;
    let a = nums(vm, ctx, k)?;
    let r: Result<(), ()> = match name.as_str() {
        "closePath" => with_path(vm, ctx, |p, _| p.close_path()).map(|_| Ok(()))?,
        "moveTo" => with_path(vm, ctx, |p, m| p.move_to(m, a[0], a[1])).map(|_| Ok(()))?,
        "lineTo" => with_path(vm, ctx, |p, m| p.line_to(m, a[0], a[1])).map(|_| Ok(()))?,
        "quadraticCurveTo" => with_path(vm, ctx, |p, m| p.quadratic_curve_to(m, a[0], a[1], a[2], a[3])).map(|_| Ok(()))?,
        "bezierCurveTo" => with_path(vm, ctx, |p, m| p.bezier_curve_to(m, a[0], a[1], a[2], a[3], a[4], a[5])).map(|_| Ok(()))?,
        "rect" => with_path(vm, ctx, |p, m| p.rect(m, a[0], a[1], a[2], a[3])).map(|_| Ok(()))?,
        "arcTo" => with_path(vm, ctx, |p, m| p.arc_to(m, a[0], a[1], a[2], a[3], a[4]))?,
        "arc" => {
            let ccw = boolean(vm, &arg(vm, ctx, 5));
            with_path(vm, ctx, |p, m| p.arc(m, a[0], a[1], a[2], a[3], a[4], ccw))?
        }
        "ellipse" => {
            let ccw = boolean(vm, &arg(vm, ctx, 7));
            with_path(vm, ctx, |p, m| p.ellipse(m, a[0], a[1], a[2], a[3], a[4], a[5], a[6], ccw))?
        }
        "roundRect" => {
            let radii = round_rect_radii(vm, &arg(vm, ctx, 4), iname)?;
            match radii {
                Some(r) => with_path(vm, ctx, |p, m| p.round_rect(m, a[0], a[1], a[2], a[3], r)).map(|_| Ok(()))?,
                None => Ok(()),
            }
        }
        _ => Ok(()),
    };
    if r.is_err() {
        let what = if name == "ellipse" { "The major-axis radius provided" } else { "The radius provided" };
        return throw_dom(vm, "IndexSizeError", &format!("Failed to execute '{name}' on '{iname}': {what} is negative."));
    }
    Ok(Value::Undefined)
}

/// roundRect's `radii` argument: `unrestricted double or DOMPointInit or sequence<…>` of 1–4 entries
/// normalised to [upper-left, upper-right, lower-right, lower-left]. Ok(None) = a non-finite value (no-op).
fn round_rect_radii(vm: &mut Vm, v: &Value, iname: &str) -> JsResult<Option<[(f64, f64); 4]>> {
    let mut list: Vec<Value> = Vec::new();
    let is_seq = v.as_object().is_some_and(|o| matches!(vm.heap.get(o).kind, Kind::Array(_)));
    if is_seq {
        let o = v.as_object().unwrap_or(Obj(0));
        let len = get(vm, o, "length")?;
        let n = vm.to_number(&len)? as usize;
        for i in 0..n.min(16) {
            list.push(vm.get(o, &PropertyKey::Index(i as u32))?);
        }
    } else if v.is_undefined() {
        list.push(Value::Number(0.0));
    } else {
        list.push(v.clone());
    }
    if list.is_empty() || list.len() > 4 {
        return vm.throw_range(&format!("Failed to execute 'roundRect' on '{iname}': {} radii provided. Between one and four radii are necessary.", list.len()));
    }
    let mut rs: Vec<(f64, f64)> = Vec::new();
    let mut bad = false;
    for r in &list {
        let (x, y) = match r {
            Value::Object(o) => {
                let x = get(vm, *o, "x")?;
                let y = get(vm, *o, "y")?;
                let x = if x.is_undefined() { 0.0 } else { vm.to_number(&x)? };
                let y = if y.is_undefined() { 0.0 } else { vm.to_number(&y)? };
                (x, y)
            }
            other => {
                let n = vm.to_number(other)?;
                (n, n)
            }
        };
        if !x.is_finite() || !y.is_finite() {
            bad = true;
        } else if x < 0.0 || y < 0.0 {
            return vm.throw_range(&format!("Failed to execute 'roundRect' on '{iname}': Radius value {} is negative.", if x < 0.0 { x } else { y }));
        }
        rs.push((x, y));
    }
    if bad {
        return Ok(None);
    }
    let r = match rs.len() {
        1 => [rs[0]; 4],
        2 => [rs[0], rs[1], rs[0], rs[1]],
        3 => [rs[0], rs[1], rs[2], rs[1]],
        _ => [rs[0], rs[1], rs[2], rs[3]],
    };
    Ok(Some(r))
}

fn path2d_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'Path2D': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    }
    let a = arg(vm, ctx, 0);
    let p = if a.is_undefined() {
        Path::new()
    } else if let Some(p) = path2d_of(vm, &a) {
        p
    } else {
        let s = string(vm, &a)?;
        parse_svg_path(&s)
    };
    let buf = new_hidden_buffer(vm, encode_path(&p))?;
    let default = iface("Path2D").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    Ok(Value::Object(host_obj(vm, proto, T_PATH, vec![Value::Object(buf)])))
}

fn path2d_add_path(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    need(vm, ctx, 1, "Path2D", "addPath")?;
    let a = arg(vm, ctx, 0);
    let Some(other) = path2d_of(vm, &a) else {
        return vm.throw_type("Failed to execute 'addPath' on 'Path2D': parameter 1 is not of type 'Path2D'.");
    };
    let t = arg(vm, ctx, 1);
    let m = matrix_from_dict(vm, &t)?;
    if !m.is_finite() {
        return Ok(Value::Undefined);
    }
    with_path(vm, ctx, |p, _| p.add_path(&other, &m))?;
    Ok(Value::Undefined)
}

/// "Create a DOMMatrix from the 2D dictionary" (Geometry Interfaces §6.1, validate and fixup 2D).
fn matrix_from_dict(vm: &mut Vm, d: &Value) -> JsResult<Matrix> {
    if d.is_nullish() {
        return Ok(Matrix::IDENTITY);
    }
    if !d.is_object() {
        return vm.throw_type("The provided value is not of type 'DOMMatrix2DInit'.");
    }
    let mut vals = [None; 12];
    for (i, name) in ["a", "b", "c", "d", "e", "f", "m11", "m12", "m21", "m22", "m41", "m42"].iter().enumerate() {
        let v = dict_member(vm, d, name)?;
        if !v.is_undefined() {
            vals[i] = Some(vm.to_number(&v)?);
        }
    }
    let mut out = [0.0; 6];
    let defaults = [1.0, 0.0, 0.0, 1.0, 0.0, 0.0];
    for k in 0..6 {
        let (short, long) = (vals[k], vals[k + 6]);
        if let (Some(x), Some(y)) = (short, long) {
            if !(x == y || (x.is_nan() && y.is_nan())) {
                return vm.throw_type("Failed to execute 'setTransform': Property mismatch on matrix initialization.");
            }
        }
        out[k] = long.or(short).unwrap_or(defaults[k]);
    }
    Ok(Matrix::new(out[0], out[1], out[2], out[3], out[4], out[5]))
}

// ------------------------------------------------------------------------------------------------ contexts

fn surface_of_ctx(vm: &Vm, o: Obj) -> u32 {
    slot_num(vm, o, 0) as u32
}

/// `this` as a context, its surface resized first if the element's width/height attributes changed.
fn this_ctx(vm: &mut Vm, ctx: &CallCtx) -> JsResult<(Obj, u32)> {
    let Some(o) = this_tagged(vm, &ctx.this, T_CTX) else { return throw_illegal(vm) };
    let id = surface_of_ctx(vm, o);
    if slot_num(vm, o, 3) == 0.0 {
        if let Some(el) = dom::node_of(vm, &slot(vm, o, 1)) {
            let (w, h) = element_size(el);
            canvas::with(id, |c| {
                let want = if w.saturating_mul(h) > canvas::context::MAX_PIXELS { (0, 0) } else { (w, h) };
                if (c.w, c.h) != want {
                    c.resize(w, h);
                }
            });
        }
    }
    Ok((o, id))
}

fn new_context(vm: &mut Vm, proto_name: &str, surface: u32, canvas_obj: Value, kind: f64) -> Obj {
    let proto = iface(proto_name).map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let keep = vm.new_array(Vec::new());
    host_obj(vm, proto, T_CTX, vec![num(surface as f64), canvas_obj, Value::Object(keep), num(kind)])
}

/// Re-collect the gradient/pattern objects the state stack references into the context's keep-alive array.
fn refresh_keepalive(vm: &mut Vm, o: Obj, id: u32) {
    let objs: Vec<Obj> = canvas::with(id, |c| {
        let mut v = Vec::new();
        for s in c.stack.iter().chain(std::iter::once(&c.state)) {
            for st in [&s.fill, &s.stroke] {
                if let Style::Object(x) = st {
                    if !v.contains(x) {
                        v.push(*x);
                    }
                }
            }
        }
        v
    })
    .unwrap_or_default();
    let arr = vm.new_array(objs.into_iter().map(Value::Object).collect());
    set_slot(vm, o, 2, Value::Object(arr));
}

fn parse_color(s: &str) -> Option<[f64; 4]> {
    let cv = css_core::parser::parse_component_values(s.trim());
    match css_core::color::parse_color(&cv)? {
        css_core::color::Color::Rgba(c) => Some([
            (c.r / 255.0).clamp(0.0, 1.0),
            (c.g / 255.0).clamp(0.0, 1.0),
            (c.b / 255.0).clamp(0.0, 1.0),
            c.a.clamp(0.0, 1.0),
        ]),
        css_core::color::Color::CurrentColor => Some([0.0, 0.0, 0.0, 1.0]),
    }
}

/// HTML "serialization of a color" for canvas: `#rrggbb` when opaque, else `rgba(r, g, b, a)`.
fn serialize_color(c: &[f64; 4]) -> String {
    let b = |v: f64| (v * 255.0).round().clamp(0.0, 255.0) as u8;
    let a8 = b(c[3]);
    if a8 == 255 {
        format!("#{:02x}{:02x}{:02x}", b(c[0]), b(c[1]), b(c[2]))
    } else {
        css_core::color::Rgba { r: b(c[0]) as f64, g: b(c[1]) as f64, b: b(c[2]) as f64, a: c[3] }.serialize()
    }
}

fn gradient_of(vm: &Vm, o: Obj) -> Gradient {
    let kind = slot_num(vm, o, 0) as u8;
    let p = hidden_bytes(vm, &slot(vm, o, 1)).map(|b| bytes_to_f64s(b)).unwrap_or_default();
    let g = |i: usize| p.get(i).copied().unwrap_or(0.0);
    let kind = match kind {
        0 => GradientKind::Linear { x0: g(0), y0: g(1), x1: g(2), y1: g(3) },
        1 => GradientKind::Radial { x0: g(0), y0: g(1), r0: g(2), x1: g(3), y1: g(4), r1: g(5) },
        _ => GradientKind::Conic { angle: g(0), x: g(1), y: g(2) },
    };
    let s = hidden_bytes(vm, &slot(vm, o, 2)).map(|b| bytes_to_f64s(b)).unwrap_or_default();
    let stops = s.chunks_exact(5).map(|c| Stop { offset: c[0], color: [c[1], c[2], c[3], c[4]] }).collect();
    Gradient { kind, stops }
}

fn pattern_paint(vm: &Vm, o: Obj, ctm: &Matrix, smooth: bool) -> Paint {
    let w = slot_num(vm, o, 0) as usize;
    let h = slot_num(vm, o, 1) as usize;
    let Some(rgba) = hidden_bytes(vm, &slot(vm, o, 2)).cloned() else { return Paint::Nothing };
    let repeat = match slot_num(vm, o, 3) as u8 {
        0 => Repeat::Both,
        1 => Repeat::X,
        2 => Repeat::Y,
        _ => Repeat::None,
    };
    let t = hidden_bytes(vm, &slot(vm, o, 4)).map(|b| bytes_to_f64s(b)).unwrap_or_default();
    let pt = if t.len() == 6 { Matrix::new(t[0], t[1], t[2], t[3], t[4], t[5]) } else { Matrix::IDENTITY };
    let Some(inv) = ctm.mul(&pt).invert() else { return Paint::Nothing };
    Paint::Image { inv, img: Bitmap { w, h, rgba: Rc::new(rgba) }, repeat, smooth, src: (0.0, 0.0, w as f64, h as f64) }
}

fn resolve_paint(vm: &Vm, style: &Style, ctm: &Matrix, smooth: bool) -> Paint {
    match style {
        Style::Color(c) => Paint::Solid(canvas::paint::premul(*c)),
        Style::Object(o) => match tag_of(vm, *o) {
            Some(T_GRAD) => match ctm.invert() {
                Some(inv) => Paint::Gradient { inv, g: gradient_of(vm, *o) },
                None => Paint::Nothing,
            },
            Some(T_PAT) => pattern_paint(vm, *o, ctm, smooth),
            _ => Paint::Nothing,
        },
    }
}

/// The paint for the fill (or stroke) style in force on surface `id`.
fn paint_for(vm: &Vm, id: u32, stroke: bool) -> Paint {
    let Some((style, ctm, smooth)) = canvas::with(id, |c| {
        (if stroke { c.state.stroke.clone() } else { c.state.fill.clone() }, c.state.ctm, c.state.smoothing)
    }) else {
        return Paint::Nothing;
    };
    resolve_paint(vm, &style, &ctm, smooth)
}

fn fill_rule(vm: &mut Vm, v: &Value, method: &str) -> JsResult<bool> {
    if v.is_undefined() {
        return Ok(false);
    }
    let s = string(vm, v)?;
    match s.as_str() {
        "nonzero" => Ok(false),
        "evenodd" => Ok(true),
        _ => vm.throw_type(&format!(
            "Failed to execute '{method}' on 'CanvasRenderingContext2D': The provided value '{s}' is not a valid enum value of type CanvasFillRule."
        )),
    }
}

fn ctx_method(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let name = callee_str(vm, ctx);
    let (o, id) = this_ctx(vm, ctx)?;
    const I: &str = "CanvasRenderingContext2D";
    match name.as_str() {
        "save" => {
            canvas::with(id, |c| c.save());
        }
        "restore" => {
            canvas::with(id, |c| c.restore());
        }
        "reset" => {
            canvas::with(id, |c| c.reset());
            refresh_keepalive(vm, o, id);
        }
        "isContextLost" => return Ok(Value::Bool(false)),
        "beginPath" => {
            canvas::with(id, |c| c.path.clear());
        }
        "scale" | "translate" => {
            need(vm, ctx, 2, I, &name)?;
            let a = nums(vm, ctx, 2)?;
            if finite(&a) {
                let m = if name == "scale" { Matrix::scale(a[0], a[1]) } else { Matrix::translate(a[0], a[1]) };
                canvas::with(id, |c| c.state.ctm = c.state.ctm.mul(&m));
            }
        }
        "rotate" => {
            need(vm, ctx, 1, I, &name)?;
            let a = num_arg(vm, ctx, 0)?;
            if a.is_finite() {
                canvas::with(id, |c| c.state.ctm = c.state.ctm.mul(&Matrix::rotate(a)));
            }
        }
        "transform" => {
            need(vm, ctx, 6, I, &name)?;
            let a = nums(vm, ctx, 6)?;
            if finite(&a) {
                let m = Matrix::new(a[0], a[1], a[2], a[3], a[4], a[5]);
                canvas::with(id, |c| c.state.ctm = c.state.ctm.mul(&m));
            }
        }
        "setTransform" => {
            let a0 = arg(vm, ctx, 0);
            if ctx.argc == 0 || a0.is_object() || (ctx.argc == 1 && a0.is_nullish()) {
                let m = matrix_from_dict(vm, &a0)?;
                if m.is_finite() {
                    canvas::with(id, |c| c.state.ctm = m);
                }
            } else {
                need(vm, ctx, 6, I, &name)?;
                let a = nums(vm, ctx, 6)?;
                if finite(&a) {
                    canvas::with(id, |c| c.state.ctm = Matrix::new(a[0], a[1], a[2], a[3], a[4], a[5]));
                }
            }
        }
        "resetTransform" => {
            canvas::with(id, |c| c.state.ctm = Matrix::IDENTITY);
        }
        "getTransform" => {
            let m = canvas::with(id, |c| c.state.ctm).unwrap_or(Matrix::IDENTITY);
            return make_dommatrix(vm, &m.to_array());
        }
        "clearRect" | "fillRect" | "strokeRect" => {
            need(vm, ctx, 4, I, &name)?;
            let a = nums(vm, ctx, 4)?;
            match name.as_str() {
                "clearRect" => {
                    canvas::with(id, |c| c.clear_rect(a[0], a[1], a[2], a[3]));
                }
                "fillRect" => {
                    let p = paint_for(vm, id, false);
                    canvas::with(id, |c| c.fill_rect(a[0], a[1], a[2], a[3], &p));
                }
                _ => {
                    let p = paint_for(vm, id, true);
                    canvas::with(id, |c| c.stroke_rect(a[0], a[1], a[2], a[3], &p));
                }
            }
        }
        "fill" | "clip" => {
            let a0 = arg(vm, ctx, 0);
            let (path, rule) = match path2d_of(vm, &a0) {
                Some(p) => (Some(p), arg(vm, ctx, 1)),
                None => {
                    if a0.is_object() && ctx.argc >= 2 {
                        return vm.throw_type(&format!("Failed to execute '{name}' on '{I}': parameter 1 is not of type 'Path2D'."));
                    }
                    (None, a0)
                }
            };
            let evenodd = fill_rule(vm, &rule, &name)?;
            let paint = if name == "fill" { Some(paint_for(vm, id, false)) } else { None };
            canvas::with(id, |c| {
                let dev = match &path {
                    Some(p) => p.transformed(&c.state.ctm),
                    None => c.path.clone(),
                };
                match &paint {
                    Some(pt) => c.fill_path(&dev, evenodd, pt),
                    None => c.clip(&dev, evenodd),
                }
            });
        }
        "stroke" => {
            let a0 = arg(vm, ctx, 0);
            let path = path2d_of(vm, &a0);
            if path.is_none() && ctx.argc >= 1 && !a0.is_undefined() {
                return vm.throw_type(&format!("Failed to execute 'stroke' on '{I}': parameter 1 is not of type 'Path2D'."));
            }
            let paint = paint_for(vm, id, true);
            canvas::with(id, |c| match &path {
                Some(p) => c.stroke_user(p, &paint),
                None => {
                    let dev = c.path.clone();
                    c.stroke_path(&dev, &paint)
                }
            });
        }
        "isPointInPath" | "isPointInStroke" => {
            let a0 = arg(vm, ctx, 0);
            let path = path2d_of(vm, &a0);
            let base = if path.is_some() { 1 } else { 0 };
            need(vm, ctx, 2 + base, I, &name)?;
            let x = num_arg(vm, ctx, base)?;
            let y = num_arg(vm, ctx, base + 1)?;
            let r = if name == "isPointInPath" {
                let rule = arg(vm, ctx, base + 2);
                let evenodd = fill_rule(vm, &rule, &name)?;
                canvas::with(id, |c| {
                    let dev = match &path {
                        Some(p) => p.transformed(&c.state.ctm),
                        None => c.path.clone(),
                    };
                    canvas::Canvas2D::point_in_path(&dev, x, y, evenodd)
                })
            } else {
                canvas::with(id, |c| {
                    let user = match &path {
                        Some(p) => Some(p.clone()),
                        None => c.state.ctm.invert().map(|inv| c.path.transformed(&inv)),
                    };
                    user.is_some_and(|u| c.point_in_stroke_user(&u, x, y))
                })
            };
            return Ok(Value::Bool(r.unwrap_or(false)));
        }
        "drawFocusIfNeeded" | "scrollPathIntoView" => {}
        "fillText" | "strokeText" => {
            need(vm, ctx, 3, I, &name)?;
            let t = string(vm, &arg(vm, ctx, 0))?;
            let x = num_arg(vm, ctx, 1)?;
            let y = num_arg(vm, ctx, 2)?;
            let mw = arg(vm, ctx, 3);
            let max_width = if mw.is_undefined() { None } else { Some(vm.to_number(&mw)?) };
            let stroke = name == "strokeText";
            let paint = paint_for(vm, id, stroke);
            canvas::with(id, |c| c.draw_text(&t, x, y, max_width, &paint, stroke));
        }
        "measureText" => {
            need(vm, ctx, 1, I, &name)?;
            let t = string(vm, &arg(vm, ctx, 0))?;
            let m = canvas::with(id, |c| c.measure_text(&t)).unwrap_or_default();
            let proto = iface("TextMetrics").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
            let vals = [
                m.width, m.actual_left, m.actual_right, m.font_ascent, m.font_descent, m.actual_ascent, m.actual_descent, m.em_ascent,
                m.em_descent, m.hanging, m.alphabetic, m.ideographic,
            ];
            return Ok(Value::Object(host_obj(vm, proto, T_METRICS, vals.iter().map(|v| num(*v)).collect())));
        }
        "setLineDash" => {
            need(vm, ctx, 1, I, &name)?;
            let a = arg(vm, ctx, 0);
            let Some(arr) = a.as_object() else {
                return vm.throw_type("Failed to execute 'setLineDash' on 'CanvasRenderingContext2D': The provided value cannot be converted to a sequence.");
            };
            let len = get(vm, arr, "length")?;
            let n = vm.to_number(&len)?;
            if !n.is_finite() || n < 0.0 {
                return vm.throw_type("Failed to execute 'setLineDash': The provided value cannot be converted to a sequence.");
            }
            let mut v = Vec::new();
            for i in 0..(n as usize).min(1 << 16) {
                let e = vm.get(arr, &PropertyKey::Index(i as u32))?;
                v.push(vm.to_number(&e)?);
            }
            if v.iter().all(|x| x.is_finite() && *x >= 0.0) {
                if v.len() % 2 == 1 {
                    let c = v.clone();
                    v.extend(c);
                }
                canvas::with(id, |c| c.state.dash = v);
            }
        }
        "getLineDash" => {
            let d = canvas::with(id, |c| c.state.dash.clone()).unwrap_or_default();
            return Ok(Value::Object(vm.new_array(d.into_iter().map(num).collect())));
        }
        "getContextAttributes" => {
            let (opaque, wr, cs) = canvas::with(id, |c| (c.opaque, c.will_read, c.color_space)).unwrap_or((false, false, "srgb"));
            let d = vm.new_plain_object();
            vm.create_data_property(d, PropertyKey::from_str("alpha"), Value::Bool(!opaque))?;
            vm.create_data_property(d, PropertyKey::from_str("colorSpace"), s(cs))?;
            vm.create_data_property(d, PropertyKey::from_str("desynchronized"), Value::Bool(false))?;
            vm.create_data_property(d, PropertyKey::from_str("willReadFrequently"), Value::Bool(wr))?;
            return Ok(Value::Object(d));
        }
        "createLinearGradient" | "createRadialGradient" | "createConicGradient" => {
            let (k, kind) = match name.as_str() {
                "createLinearGradient" => (4, 0.0),
                "createRadialGradient" => (6, 1.0),
                _ => (3, 2.0),
            };
            need(vm, ctx, k, I, &name)?;
            let a = nums(vm, ctx, k)?;
            if !finite(&a) {
                return non_finite(vm, I, &name);
            }
            if kind == 1.0 && (a[2] < 0.0 || a[5] < 0.0) {
                return throw_dom(vm, "IndexSizeError", &format!("Failed to execute 'createRadialGradient' on '{I}': The {} radius provided ({}) is negative.", if a[2] < 0.0 { "r0" } else { "r1" }, if a[2] < 0.0 { a[2] } else { a[5] }));
            }
            let params = new_hidden_buffer(vm, f64s_to_bytes(&a))?;
            let stops = new_hidden_buffer(vm, Vec::new())?;
            let proto = iface("CanvasGradient").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
            return Ok(Value::Object(host_obj(vm, proto, T_GRAD, vec![num(kind), Value::Object(params), Value::Object(stops)])));
        }
        "createPattern" => {
            need(vm, ctx, 2, I, &name)?;
            let src = arg(vm, ctx, 0);
            let rep = arg(vm, ctx, 1);
            let rep = if rep.is_null() { String::new() } else { string(vm, &rep)? };
            let img = image_source(vm, &src, I, &name)?;
            let Some(img) = img else { return Ok(Value::Null) };
            let repeat = match rep.as_str() {
                "" | "repeat" => 0.0,
                "repeat-x" => 1.0,
                "repeat-y" => 2.0,
                "no-repeat" => 3.0,
                _ => {
                    return throw_dom(vm, "SyntaxError", &format!("Failed to execute 'createPattern' on '{I}': The provided type ('{rep}') is not one of 'repeat', 'no-repeat', 'repeat-x', or 'repeat-y'."));
                }
            };
            let data = new_hidden_buffer(vm, (*img.rgba).clone())?;
            let t = new_hidden_buffer(vm, f64s_to_bytes(&Matrix::IDENTITY.to_array()))?;
            let proto = iface("CanvasPattern").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
            return Ok(Value::Object(host_obj(vm, proto, T_PAT, vec![num(img.w as f64), num(img.h as f64), Value::Object(data), num(repeat), Value::Object(t)])));
        }
        "drawImage" => return draw_image(vm, ctx, id),
        "createImageData" => {
            need(vm, ctx, 1, I, &name)?;
            let a0 = arg(vm, ctx, 0);
            let (w, h) = if let Some(src) = a0.as_object().filter(|o| tag_of(vm, *o) == Some(T_IDATA)) {
                (slot_num(vm, src, 0) as usize, slot_num(vm, src, 1) as usize)
            } else {
                need(vm, ctx, 2, I, &name)?;
                let w = enforce_long(vm, &a0, I, &name)?;
                let h = enforce_long(vm, &arg(vm, ctx, 1), I, &name)?;
                if w == 0 || h == 0 {
                    return throw_dom(vm, "IndexSizeError", &format!("Failed to execute 'createImageData' on '{I}': The source {} is 0.", if w == 0 { "width" } else { "height" }));
                }
                (w.unsigned_abs() as usize, h.unsigned_abs() as usize)
            };
            return new_image_data(vm, w, h, None);
        }
        "getImageData" => {
            need(vm, ctx, 4, I, &name)?;
            let mut v = [0i64; 4];
            for (i, slot_v) in v.iter_mut().enumerate() {
                *slot_v = enforce_long(vm, &arg(vm, ctx, i), I, &name)?;
            }
            let [mut sx, mut sy, mut sw, mut sh] = v;
            if sw == 0 || sh == 0 {
                return throw_dom(vm, "IndexSizeError", &format!("Failed to execute 'getImageData' on '{I}': The source {} is 0.", if sw == 0 { "width" } else { "height" }));
            }
            if sw < 0 {
                sx += sw;
                sw = -sw;
            }
            if sh < 0 {
                sy += sh;
                sh = -sh;
            }
            if (sw as u64).saturating_mul(sh as u64) > canvas::context::MAX_PIXELS as u64 {
                return vm.throw_range("Out of memory at ImageData creation");
            }
            let data = canvas::with(id, |c| c.straight_rect(sx, sy, sw, sh)).unwrap_or_default();
            return new_image_data(vm, sw as usize, sh as usize, Some(data));
        }
        "putImageData" => {
            need(vm, ctx, 3, I, &name)?;
            let d = arg(vm, ctx, 0);
            let Some(io) = d.as_object().filter(|o| tag_of(vm, *o) == Some(T_IDATA)) else {
                return vm.throw_type(&format!("Failed to execute 'putImageData' on '{I}': parameter 1 is not of type 'ImageData'."));
            };
            let dx = enforce_long(vm, &arg(vm, ctx, 1), I, &name)?;
            let dy = enforce_long(vm, &arg(vm, ctx, 2), I, &name)?;
            let iw = slot_num(vm, io, 0) as i64;
            let ih = slot_num(vm, io, 1) as i64;
            let (mut rx, mut ry, mut rw, mut rh) = (0, 0, iw, ih);
            if ctx.argc >= 7 {
                rx = enforce_long(vm, &arg(vm, ctx, 3), I, &name)?;
                ry = enforce_long(vm, &arg(vm, ctx, 4), I, &name)?;
                rw = enforce_long(vm, &arg(vm, ctx, 5), I, &name)?;
                rh = enforce_long(vm, &arg(vm, ctx, 6), I, &name)?;
            } else if ctx.argc > 3 {
                return vm.throw_type(&format!("Failed to execute 'putImageData' on '{I}': Valid arities are: [3, 7], but {} arguments provided.", ctx.argc));
            }
            let dv = slot(vm, io, 2);
            let bytes = super::net::buffer_bytes(vm, &dv).unwrap_or_default();
            if bytes.is_empty() {
                return throw_dom(vm, "InvalidStateError", &format!("Failed to execute 'putImageData' on '{I}': The source data has been detached."));
            }
            if rw < 0 {
                rx += rw;
                rw = -rw;
            }
            if rh < 0 {
                ry += rh;
                rh = -rh;
            }
            if rx < 0 {
                rw += rx;
                rx = 0;
            }
            if ry < 0 {
                rh += ry;
                ry = 0;
            }
            if rw > 0 && rh > 0 {
                canvas::with(id, |c| c.put_image_data(&bytes, iw as usize, ih as usize, dx, dy, rx, ry, rw, rh));
            }
        }
        _ => {}
    }
    Ok(Value::Undefined)
}

/// `[EnforceRange] long`.
fn enforce_long(vm: &mut Vm, v: &Value, iname: &str, method: &str) -> JsResult<i64> {
    let n = vm.to_number(v)?;
    if !n.is_finite() {
        return vm.throw_type(&format!("Failed to execute '{method}' on '{iname}': Value is not of type 'long'."));
    }
    let t = n.trunc();
    if !(-2_147_483_648.0..=2_147_483_647.0).contains(&t) {
        return vm.throw_type(&format!("Failed to execute '{method}' on '{iname}': Value is outside the 'long' value range."));
    }
    Ok(t as i64)
}

fn make_dommatrix(vm: &mut Vm, v: &[f64; 6]) -> JsResult<Value> {
    let arr = vm.new_array(v.iter().map(|x| num(*x)).collect());
    match DOMMATRIX.with(|d| d.get()) {
        Some(r) => {
            let c = rooted(vm, r);
            vm.construct(&c, &[Value::Object(arr)], None)
        }
        None => {
            // No DOMMatrix (prelude failed): a plain object with the six members.
            let o = vm.new_plain_object();
            for (k, n) in ["a", "b", "c", "d", "e", "f"].iter().enumerate() {
                vm.create_data_property(o, PropertyKey::from_str(n), num(v[k]))?;
            }
            Ok(Value::Object(o))
        }
    }
}

// ------------------------------------------------------------------------------------------------ images

/// A CanvasImageSource (or, for createImageBitmap, also ImageData / encoded bytes). Ok(None) = not fully
/// available (draw nothing, createPattern → null). Err = TypeError (not a source) or InvalidStateError.
fn image_source(vm: &mut Vm, v: &Value, iname: &str, method: &str) -> JsResult<Option<Bitmap>> {
    let type_err = |vm: &mut Vm| -> JsResult<Option<Bitmap>> {
        vm.throw_type(&format!(
            "Failed to execute '{method}' on '{iname}': The provided value is not of type '(CSSImageValue or HTMLCanvasElement or HTMLImageElement or HTMLVideoElement or ImageBitmap or OffscreenCanvas or SVGImageElement or VideoFrame)'."
        ))
    };
    let Some(o) = v.as_object() else { return type_err(vm) };
    if let Some(el) = dom::node_of(vm, v) {
        if tag_is(el, "img") {
            let src = attr_value(el, "src").and_then(|x| dom::resolve_url(dom::node_document(el), &x));
            let Some(img) = src.and_then(|u| crate::images::get(&u)) else { return Ok(None) };
            return Ok(Some(Bitmap { w: img.width() as usize, h: img.height() as usize, rgba: Rc::new(img.as_raw().clone()) }));
        }
        if tag_is(el, "canvas") {
            let (w, h) = element_size(el);
            let snap = canvas::element_surface(el).and_then(|id| canvas::with(id, |c| ((c.w, c.h) == (w, h)).then(|| c.snapshot())).flatten());
            if w == 0 || h == 0 {
                return throw_dom(vm, "InvalidStateError", &format!("Failed to execute '{method}' on '{iname}': The image argument is a canvas element with a width or height of 0."));
            }
            return Ok(Some(snap.unwrap_or_else(|| Bitmap { w, h, rgba: Rc::new(vec![0u8; w * h * 4]) })));
        }
        if tag_is(el, "video") {
            let node = dom::node_ref(el);
            let pic = crate::media::paint_for(&node).and_then(|p| p.picture);
            return Ok(pic.map(|img| Bitmap { w: img.width() as usize, h: img.height() as usize, rgba: Rc::new(img.as_raw().clone()) }));
        }
        let svg_image = with_doc(|d| d.element(NodeId(el)).is_some_and(|e| e.local.as_str() == "image"));
        if svg_image {
            return Ok(None);
        }
        return type_err(vm);
    }
    match tag_of(vm, o) {
        Some(T_OFF) => {
            let (w, h) = (slot_num(vm, o, 1) as usize, slot_num(vm, o, 2) as usize);
            if w == 0 || h == 0 {
                return throw_dom(vm, "InvalidStateError", &format!("Failed to execute '{method}' on '{iname}': The image argument is an OffscreenCanvas element with a width or height of 0."));
            }
            let sid = slot_num(vm, o, 0);
            let snap = if sid >= 0.0 { canvas::with(sid as u32, |c| c.snapshot()) } else { None };
            Ok(Some(snap.unwrap_or_else(|| Bitmap { w, h, rgba: Rc::new(vec![0u8; w * h * 4]) })))
        }
        Some(T_BMP) => {
            let (w, h) = (slot_num(vm, o, 0) as usize, slot_num(vm, o, 1) as usize);
            match hidden_bytes(vm, &slot(vm, o, 2)) {
                Some(b) => Ok(Some(Bitmap { w, h, rgba: Rc::new(b.clone()) })),
                None => throw_dom(vm, "InvalidStateError", &format!("Failed to execute '{method}' on '{iname}': The image source is detached.")),
            }
        }
        _ => type_err(vm),
    }
}

fn draw_image(vm: &mut Vm, ctx: &CallCtx, id: u32) -> JsResult<Value> {
    const I: &str = "CanvasRenderingContext2D";
    need(vm, ctx, 3, I, "drawImage")?;
    if !matches!(ctx.argc, 3 | 5 | 9) {
        return vm.throw_type(&format!("Failed to execute 'drawImage' on '{I}': Valid arities are: [3, 5, 9], but {} arguments provided.", ctx.argc));
    }
    let src = arg(vm, ctx, 0);
    let img = image_source(vm, &src, I, "drawImage")?;
    let mut a = Vec::with_capacity(8);
    for i in 1..ctx.argc {
        a.push(num_arg(vm, ctx, i)?);
    }
    let Some(img) = img else { return Ok(Value::Undefined) };
    if !finite(&a) {
        return Ok(Value::Undefined);
    }
    let (iw, ih) = (img.w as f64, img.h as f64);
    let (mut sx, mut sy, mut sw, mut sh, mut dx, mut dy, mut dw, mut dh) = match ctx.argc {
        3 => (0.0, 0.0, iw, ih, a[0], a[1], iw, ih),
        5 => (0.0, 0.0, iw, ih, a[0], a[1], a[2], a[3]),
        _ => (a[0], a[1], a[2], a[3], a[4], a[5], a[6], a[7]),
    };
    if sw < 0.0 {
        sx += sw;
        sw = -sw;
    }
    if sh < 0.0 {
        sy += sh;
        sh = -sh;
    }
    if dw < 0.0 {
        dx += dw;
        dw = -dw;
    }
    if dh < 0.0 {
        dy += dh;
        dh = -dh;
    }
    if sw == 0.0 || sh == 0.0 || dw == 0.0 || dh == 0.0 || iw == 0.0 || ih == 0.0 {
        return Ok(Value::Undefined);
    }
    // Clip the source rectangle to the image; the destination shrinks in proportion.
    let (kx, ky) = (dw / sw, dh / sh);
    let nx0 = sx.max(0.0);
    let ny0 = sy.max(0.0);
    let nx1 = (sx + sw).min(iw);
    let ny1 = (sy + sh).min(ih);
    if nx1 <= nx0 || ny1 <= ny0 {
        return Ok(Value::Undefined);
    }
    dx += (nx0 - sx) * kx;
    dy += (ny0 - sy) * ky;
    dw = (nx1 - nx0) * kx;
    dh = (ny1 - ny0) * ky;
    canvas::with(id, |c| c.draw_image(img, nx0, ny0, nx1 - nx0, ny1 - ny0, dx, dy, dw, dh));
    Ok(Value::Undefined)
}

// ------------------------------------------------------------------------------------------------ attributes

fn ctx_attr_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let name = callee_str(vm, ctx);
    let (o, id) = this_ctx(vm, ctx)?;
    if name == "canvas" {
        return Ok(slot(vm, o, 1));
    }
    enum G {
        N(f64),
        S(String),
        B(bool),
        Style(Style),
    }
    let g = canvas::with(id, |c| {
        let s = &c.state;
        match name.as_str() {
            "globalAlpha" => G::N(s.alpha),
            "globalCompositeOperation" => G::S(s.op.name().into()),
            "imageSmoothingEnabled" => G::B(s.smoothing),
            "imageSmoothingQuality" => G::S(s.smoothing_quality.into()),
            "strokeStyle" => G::Style(s.stroke.clone()),
            "fillStyle" => G::Style(s.fill.clone()),
            "shadowOffsetX" => G::N(s.shadow_x),
            "shadowOffsetY" => G::N(s.shadow_y),
            "shadowBlur" => G::N(s.shadow_blur),
            "shadowColor" => G::S(serialize_color(&s.shadow_color)),
            "filter" => G::S(s.filter.clone()),
            "lineWidth" => G::N(s.line_width),
            "lineCap" => G::S(match s.cap {
                Cap::Butt => "butt",
                Cap::Round => "round",
                Cap::Square => "square",
            }
            .into()),
            "lineJoin" => G::S(match s.join {
                Join::Round => "round",
                Join::Bevel => "bevel",
                Join::Miter => "miter",
            }
            .into()),
            "miterLimit" => G::N(s.miter),
            "lineDashOffset" => G::N(s.dash_offset),
            "font" => G::S(text::serialize_font(&s.font)),
            "textAlign" => G::S(match s.align {
                Align::Start => "start",
                Align::End => "end",
                Align::Left => "left",
                Align::Right => "right",
                Align::Center => "center",
            }
            .into()),
            "textBaseline" => G::S(match s.baseline {
                Baseline::Top => "top",
                Baseline::Hanging => "hanging",
                Baseline::Middle => "middle",
                Baseline::Alphabetic => "alphabetic",
                Baseline::Ideographic => "ideographic",
                Baseline::Bottom => "bottom",
            }
            .into()),
            "direction" => G::S(["inherit", "ltr", "rtl"][s.direction as usize % 3].into()),
            "letterSpacing" => G::S(s.letter_spacing.1.clone()),
            "wordSpacing" => G::S(s.word_spacing.1.clone()),
            "fontKerning" => G::S(s.kerning.into()),
            "fontStretch" => G::S(text::stretch_name(s.stretch).into()),
            "fontVariantCaps" => G::S(s.variant_caps.into()),
            "textRendering" => G::S(s.text_rendering.into()),
            _ => G::N(0.0),
        }
    });
    Ok(match g {
        Some(G::N(n)) => num(n),
        Some(G::S(t)) => s(&t),
        Some(G::B(b)) => Value::Bool(b),
        Some(G::Style(Style::Color(c))) => s(&serialize_color(&c)),
        Some(G::Style(Style::Object(x))) => Value::Object(x),
        None => Value::Undefined,
    })
}

const KEYWORDS: &[(&str, &[&str])] = &[
    ("imageSmoothingQuality", &["low", "medium", "high"]),
    ("lineCap", &["butt", "round", "square"]),
    ("lineJoin", &["round", "bevel", "miter"]),
    ("textAlign", &["start", "end", "left", "right", "center"]),
    ("textBaseline", &["top", "hanging", "middle", "alphabetic", "ideographic", "bottom"]),
    ("direction", &["ltr", "rtl", "inherit"]),
    ("fontKerning", &["auto", "normal", "none"]),
    (
        "fontStretch",
        &["ultra-condensed", "extra-condensed", "condensed", "semi-condensed", "normal", "semi-expanded", "expanded", "extra-expanded", "ultra-expanded"],
    ),
    ("fontVariantCaps", &["normal", "small-caps", "all-small-caps", "petite-caps", "all-petite-caps", "unicase", "titling-caps"]),
    ("textRendering", &["auto", "optimizeSpeed", "optimizeLegibility", "geometricPrecision"]),
];

/// A CSS `<length>` for letterSpacing/wordSpacing, in px (em against the current font size).
fn css_length(t: &str, font_px: f64) -> Option<f64> {
    let t = t.trim().to_ascii_lowercase();
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '+' || c == '-')).unwrap_or(t.len());
    let (n, unit) = t.split_at(split);
    let v: f64 = n.parse().ok()?;
    let px = match unit {
        "px" => v,
        "pt" => v * 4.0 / 3.0,
        "pc" => v * 16.0,
        "in" => v * 96.0,
        "cm" => v * 96.0 / 2.54,
        "mm" => v * 96.0 / 25.4,
        "em" => v * font_px,
        "rem" => v * 16.0,
        "ex" | "ch" => v * font_px * 0.5,
        "" if v == 0.0 => 0.0,
        _ => return None,
    };
    px.is_finite().then_some(px)
}

fn ctx_attr_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let name = callee_str(vm, ctx);
    let (o, id) = this_ctx(vm, ctx)?;
    let v = arg(vm, ctx, 0);
    match name.as_str() {
        "globalAlpha" | "shadowOffsetX" | "shadowOffsetY" | "shadowBlur" | "lineWidth" | "miterLimit" | "lineDashOffset" => {
            let n = vm.to_number(&v)?;
            if !n.is_finite() {
                return Ok(Value::Undefined);
            }
            canvas::with(id, |c| {
                let s = &mut c.state;
                match name.as_str() {
                    "globalAlpha" if (0.0..=1.0).contains(&n) => s.alpha = n,
                    "shadowOffsetX" => s.shadow_x = n,
                    "shadowOffsetY" => s.shadow_y = n,
                    "shadowBlur" if n >= 0.0 => s.shadow_blur = n,
                    "lineWidth" if n > 0.0 => s.line_width = n,
                    "miterLimit" if n > 0.0 => s.miter = n,
                    "lineDashOffset" => s.dash_offset = n,
                    _ => {}
                }
            });
        }
        "imageSmoothingEnabled" => {
            let b = boolean(vm, &v);
            canvas::with(id, |c| c.state.smoothing = b);
        }
        "fillStyle" | "strokeStyle" => {
            let style = match v.as_object().filter(|x| matches!(tag_of(vm, *x), Some(T_GRAD) | Some(T_PAT))) {
                Some(x) => Some(Style::Object(x)),
                None => {
                    let t = string(vm, &v)?;
                    parse_color(&t).map(Style::Color)
                }
            };
            if let Some(st) = style {
                canvas::with(id, |c| {
                    if name == "fillStyle" {
                        c.state.fill = st;
                    } else {
                        c.state.stroke = st;
                    }
                });
                refresh_keepalive(vm, o, id);
            }
        }
        "shadowColor" => {
            let t = string(vm, &v)?;
            if let Some(c) = parse_color(&t) {
                canvas::with(id, |cv| cv.state.shadow_color = c);
            }
        }
        "globalCompositeOperation" => {
            let t = string(vm, &v)?;
            if let Some(op) = Op::parse(&t) {
                canvas::with(id, |c| c.state.op = op);
            }
        }
        "filter" => {
            // AETHERDEBT SR93: the value is a CSS <filter-value-list> (or `none`), applied by the drawing
            // model (`canvas::filter`); one that does not parse is ignored.
            let t = string(vm, &v)?;
            if canvas::filter::parse(&t).is_some() {
                canvas::with(id, |c| c.state.filter = t);
            }
        }
        "font" => {
            let t = string(vm, &v)?;
            if let Some(f) = text::parse_font(&t) {
                canvas::with(id, |c| c.state.font = f);
            }
        }
        "letterSpacing" | "wordSpacing" => {
            let t = string(vm, &v)?;
            canvas::with(id, |c| {
                if let Some(px) = css_length(&t, c.state.font.size) {
                    let val = (px, t.trim().to_ascii_lowercase());
                    if name == "letterSpacing" {
                        c.state.letter_spacing = val;
                    } else {
                        c.state.word_spacing = val;
                    }
                }
            });
        }
        _ => {
            // Enumerated attributes: an invalid value is ignored (WebIDL enum attribute assignment).
            let t = string(vm, &v)?;
            let Some((_, allowed)) = KEYWORDS.iter().find(|(n, _)| *n == name) else { return Ok(Value::Undefined) };
            let Some(kw) = allowed.iter().find(|k| **k == t).copied() else { return Ok(Value::Undefined) };
            canvas::with(id, |c| {
                let s = &mut c.state;
                match name.as_str() {
                    "imageSmoothingQuality" => s.smoothing_quality = kw,
                    "lineCap" => {
                        s.cap = match kw {
                            "round" => Cap::Round,
                            "square" => Cap::Square,
                            _ => Cap::Butt,
                        }
                    }
                    "lineJoin" => {
                        s.join = match kw {
                            "round" => Join::Round,
                            "bevel" => Join::Bevel,
                            _ => Join::Miter,
                        }
                    }
                    "textAlign" => {
                        s.align = match kw {
                            "end" => Align::End,
                            "left" => Align::Left,
                            "right" => Align::Right,
                            "center" => Align::Center,
                            _ => Align::Start,
                        }
                    }
                    "textBaseline" => {
                        s.baseline = match kw {
                            "top" => Baseline::Top,
                            "hanging" => Baseline::Hanging,
                            "middle" => Baseline::Middle,
                            "ideographic" => Baseline::Ideographic,
                            "bottom" => Baseline::Bottom,
                            _ => Baseline::Alphabetic,
                        }
                    }
                    "direction" => {
                        s.direction = match kw {
                            "ltr" => 1,
                            "rtl" => 2,
                            _ => 0,
                        }
                    }
                    "fontKerning" => s.kerning = kw,
                    "fontStretch" => s.stretch = text::stretch_value(kw).unwrap_or(100),
                    "fontVariantCaps" => s.variant_caps = kw,
                    "textRendering" => s.text_rendering = kw,
                    _ => {}
                }
            });
        }
    }
    Ok(Value::Undefined)
}

// ------------------------------------------------------------------------------------------------ gradients, patterns, metrics, ImageData

fn gradient_add_stop(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_GRAD) else { return throw_illegal(vm) };
    need(vm, ctx, 2, "CanvasGradient", "addColorStop")?;
    let off = num_arg(vm, ctx, 0)?;
    let color = string(vm, &arg(vm, ctx, 1))?;
    if !off.is_finite() {
        return non_finite(vm, "CanvasGradient", "addColorStop");
    }
    if !(0.0..=1.0).contains(&off) {
        return throw_dom(vm, "IndexSizeError", &format!("Failed to execute 'addColorStop' on 'CanvasGradient': The provided value ({off}) is outside the range (0.0, 1.0)."));
    }
    let Some(c) = parse_color(&color) else {
        return throw_dom(vm, "SyntaxError", &format!("Failed to execute 'addColorStop' on 'CanvasGradient': The value provided ('{color}') could not be parsed as a color."));
    };
    let sv = slot(vm, o, 2);
    let mut stops = hidden_bytes(vm, &sv).map(|b| bytes_to_f64s(b)).unwrap_or_default();
    // Keep stops sorted by offset; equal offsets stay in insertion order.
    let mut at = stops.len() / 5;
    for k in 0..stops.len() / 5 {
        if stops[k * 5] > off {
            at = k;
            break;
        }
    }
    let rec = [off, c[0], c[1], c[2], c[3]];
    for (j, x) in rec.iter().enumerate() {
        stops.insert(at * 5 + j, *x);
    }
    if let Some(b) = hidden_bytes_mut(vm, &sv) {
        *b = f64s_to_bytes(&stops);
    }
    Ok(Value::Undefined)
}

fn pattern_set_transform(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_PAT) else { return throw_illegal(vm) };
    let m = matrix_from_dict(vm, &arg(vm, ctx, 0))?;
    if m.is_finite() {
        let tv = slot(vm, o, 4);
        if let Some(b) = hidden_bytes_mut(vm, &tv) {
            *b = f64s_to_bytes(&m.to_array());
        }
    }
    Ok(Value::Undefined)
}

fn metrics_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_METRICS) else { return throw_illegal(vm) };
    let i: usize = callee_str(vm, ctx).parse().unwrap_or(0);
    Ok(slot(vm, o, i))
}

fn new_image_data(vm: &mut Vm, w: usize, h: usize, data: Option<Vec<u8>>) -> JsResult<Value> {
    let len = w.checked_mul(h).and_then(|p| p.checked_mul(4)).filter(|l| *l <= (1 << 30));
    let Some(len) = len else { return vm.throw_range("Out of memory at ImageData creation") };
    let bytes = data.unwrap_or_else(|| vec![0u8; len]);
    let arr = clamped_array(vm, bytes)?;
    let proto = iface("ImageData").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    Ok(Value::Object(host_obj(vm, proto, T_IDATA, vec![num(w as f64), num(h as f64), Value::Object(arr), s("srgb")])))
}

fn image_data_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    const I: &str = "ImageData";
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'ImageData': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    }
    need(vm, ctx, 2, I, "constructor")?;
    let a0 = arg(vm, ctx, 0);
    let (w, h, data, arr) = if is_clamped_array(vm, &a0) {
        let bytes = super::net::buffer_bytes(vm, &a0).unwrap_or_default();
        let sw = vm.to_uint32(&arg(vm, ctx, 1))? as usize;
        if bytes.is_empty() || bytes.len() % 4 != 0 {
            return throw_dom(vm, "InvalidStateError", "Failed to construct 'ImageData': The input data has zero elements, or the length is not a multiple of 4.");
        }
        if sw == 0 || (bytes.len() / 4) % sw != 0 {
            return throw_dom(vm, "IndexSizeError", "Failed to construct 'ImageData': The input data length is not a multiple of (4 * width).");
        }
        let h = bytes.len() / 4 / sw;
        let hv = arg(vm, ctx, 2);
        if !hv.is_undefined() {
            let sh = vm.to_uint32(&hv)? as usize;
            if sh != h {
                return throw_dom(vm, "IndexSizeError", "Failed to construct 'ImageData': The input data length is not equal to (4 * width * height).");
            }
        }
        (sw, h, None, a0.as_object())
    } else {
        if a0.is_object() && ctx.argc >= 3 && !arg(vm, ctx, 2).is_object() {
            return vm.throw_type("Failed to construct 'ImageData': parameter 1 is not of type 'Uint8ClampedArray'.");
        }
        let sw = vm.to_uint32(&a0)? as usize;
        let sh = vm.to_uint32(&arg(vm, ctx, 1))? as usize;
        if sw == 0 || sh == 0 {
            return throw_dom(vm, "IndexSizeError", &format!("Failed to construct 'ImageData': The source {} is zero or not a number.", if sw == 0 { "width" } else { "height" }));
        }
        if sw.saturating_mul(sh) > (1 << 28) {
            return vm.throw_range("Failed to construct 'ImageData': Out of memory at ImageData creation");
        }
        (sw, sh, Some(vec![0u8; sw * sh * 4]), None)
    };
    let arr = match arr {
        Some(a) => a,
        None => clamped_array(vm, data.unwrap_or_default())?,
    };
    let default = iface("ImageData").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    Ok(Value::Object(host_obj(vm, proto, T_IDATA, vec![num(w as f64), num(h as f64), Value::Object(arr), s("srgb")])))
}

fn image_data_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_IDATA) else { return throw_illegal(vm) };
    let i: usize = callee_str(vm, ctx).parse().unwrap_or(0);
    Ok(slot(vm, o, i))
}

// ------------------------------------------------------------------------------------------------ HTMLCanvasElement

fn this_canvas(vm: &mut Vm, ctx: &CallCtx) -> JsResult<usize> {
    let el = dom::this_element(vm, ctx)?;
    if !is_canvas(el) {
        return throw_illegal(vm);
    }
    Ok(el)
}

struct Settings {
    alpha: bool,
    will_read: bool,
    color_space: &'static str,
}

fn context_settings(vm: &mut Vm, v: &Value) -> JsResult<Settings> {
    let mut st = Settings { alpha: true, will_read: false, color_space: "srgb" };
    if v.is_nullish() {
        return Ok(st);
    }
    if !v.is_object() {
        return Ok(st);
    }
    let a = dict_member(vm, v, "alpha")?;
    if !a.is_undefined() {
        st.alpha = boolean(vm, &a);
    }
    let w = dict_member(vm, v, "willReadFrequently")?;
    if !w.is_undefined() {
        st.will_read = boolean(vm, &w);
    }
    let c = dict_member(vm, v, "colorSpace")?;
    if !c.is_undefined() {
        let t = string(vm, &c)?;
        st.color_space = match t.as_str() {
            "srgb" => "srgb",
            "display-p3" => "display-p3",
            _ => return vm.throw_type(&format!("Failed to execute 'getContext': The provided value '{t}' is not a valid enum value of type PredefinedColorSpace.")),
        };
    }
    Ok(st)
}

fn canvas_get_context(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_canvas(vm, ctx)?;
    need(vm, ctx, 1, "HTMLCanvasElement", "getContext")?;
    let id = string(vm, &arg(vm, ctx, 0))?;
    // WEBGL1 (SR136): one context type per canvas — a canvas holding one kind answers null for the other.
    if matches!(id.as_str(), "webgl" | "experimental-webgl") {
        if page(|p| p.same_object.contains_key(&(el, SO_CTX))) {
            return Ok(Value::Null);
        }
        let attrs = arg(vm, ctx, 1);
        return super::webgl::element_context(vm, el, &attrs);
    }
    if id == "2d" && page(|p| p.same_object.contains_key(&(el, super::webgl::SO_GL))) {
        return Ok(Value::Null);
    }
    if id != "2d" {
        if matches!(id.as_str(), "webgl2" | "webgpu" | "bitmaprenderer") {
            crate::ledger::record_dom(&format!("canvas-context-refused:{id}"));
        }
        return Ok(Value::Null);
    }
    if let Some(o) = page(|p| p.same_object.get(&(el, SO_CTX)).map(|x| x.0)) {
        return Ok(Value::Object(o));
    }
    let st = context_settings(vm, &arg(vm, ctx, 1))?;
    let (w, h) = element_size(el);
    let sid = canvas::create(w, h, !st.alpha);
    canvas::with(sid, |c| {
        c.will_read = st.will_read;
        c.color_space = st.color_space;
    });
    canvas::bind_element(el, sid);
    let wrapper = dom::wrap(vm, el);
    let o = new_context(vm, "CanvasRenderingContext2D", sid, wrapper, 0.0);
    let r = root(vm, Value::Object(o));
    page(|p| p.same_object.insert((el, SO_CTX), (o, r)));
    Ok(Value::Object(o))
}

/// PNG bytes of an element canvas (blank when it has no context), None for a 0-sized canvas.
fn element_png(el: usize) -> Option<Vec<u8>> {
    let (w, h) = element_size(el);
    if w == 0 || h == 0 {
        return None;
    }
    if let Some(sid) = canvas::element_surface(el) {
        if let Some(Some(png)) = canvas::with(sid, |c| ((c.w, c.h) == (w, h)).then(|| c.to_png()).flatten()) {
            return Some(png);
        }
    }
    if w.saturating_mul(h) > canvas::context::MAX_PIXELS {
        return None;
    }
    pixel_core::png::encode::encode_rgba(w as u32, h as u32, &vec![0u8; w * h * 4]).ok()
}

fn data_url(png: Option<Vec<u8>>) -> String {
    use base64::Engine as _;
    match png {
        Some(b) => format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(b)),
        None => "data:,".into(),
    }
}

fn canvas_to_data_url(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_canvas(vm, ctx)?;
    let t = arg(vm, ctx, 0);
    if !t.is_undefined() {
        let ty = string(vm, &t)?.to_ascii_lowercase();
        if ty != "image/png" {
            // pixel_core has a PNG encoder only; the spec's fallback for an unsupported type is PNG.
            crate::ledger::record_dom(&format!("canvas-encode-fallback-png:{}", super::clip(&ty, 24)));
        }
    }
    Ok(s(&data_url(element_png(el))))
}

fn uint8_array(vm: &mut Vm, bytes: Vec<u8>) -> JsResult<Value> {
    let len = bytes.len();
    let b = new_array_buffer(vm, 0)?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(b).kind {
        d.data = bytes;
    }
    let ki = KINDS.iter().position(|k| *k == TAKind::Uint8).unwrap_or(1);
    let proto = vm.intr().ta_protos[ki];
    Ok(Value::Object(vm.alloc(ObjectData::new(
        Some(proto),
        Kind::TypedArray(Box::new(TypedArrayData { kind: TAKind::Uint8, buffer: b, byte_offset: 0, length: Some(len) })),
    ))))
}

/// Prelude helper: PNG bytes (Uint8Array) of a canvas element or OffscreenCanvas, or null when 0-sized.
fn canvas_png_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let png = if let Some(el) = dom::node_of(vm, &v).filter(|e| is_canvas(*e)) {
        element_png(el)
    } else if let Some(o) = v.as_object().filter(|o| tag_of(vm, *o) == Some(T_OFF)) {
        offscreen_png(vm, o)
    } else {
        return vm.throw_type("Illegal invocation");
    };
    match png {
        Some(b) => uint8_array(vm, b),
        None => Ok(Value::Null),
    }
}

fn canvas_dim_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_canvas(vm, ctx)?;
    let (w, h) = element_size(el);
    Ok(num(if callee_str(vm, ctx) == "width" { w } else { h } as f64))
}

fn canvas_dim_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let el = this_canvas(vm, ctx)?;
    let which = callee_str(vm, ctx);
    let v = vm.to_uint32(&arg(vm, ctx, 0))?;
    let v = if v > 2_147_483_647 { if which == "width" { 300 } else { 150 } } else { v };
    // HTML: setting width or height (even to its current value) resets the bitmap and the context —
    // the attribute change steps do it ([`dimension_attribute_changed`]), for setAttribute too.
    set_attr(vm, el, &which, &v.to_string());
    Ok(Value::Undefined)
}

/// The canvas element's attribute change steps for `width` / `height` (HTML §4.12.5: "whenever the
/// width and height content attributes are set, removed, changed, or redundantly set to the value
/// they already have", the bitmap and the context reset — AETHERDEBT SR93; `js/dom.rs`
/// `attribute_changed` calls this).
pub fn dimension_attribute_changed(vm: &mut Vm, el: usize) {
    if let Some(sid) = canvas::element_surface(el) {
        let (w, h) = element_size(el);
        canvas::with(sid, |c| c.resize(w, h));
        if let Some(o) = page(|p| p.same_object.get(&(el, SO_CTX)).map(|x| x.0)) {
            refresh_keepalive(vm, o, sid);
        }
    }
}

// ------------------------------------------------------------------------------------------------ OffscreenCanvas, ImageBitmap

fn enforce_ull(vm: &mut Vm, v: &Value, what: &str) -> JsResult<usize> {
    let n = vm.to_number(v)?;
    if !n.is_finite() || n.trunc() < 0.0 || n.trunc() > 9_007_199_254_740_991.0 {
        return vm.throw_type(&format!("Failed to {what}: Value is outside the 'unsigned long long' value range."));
    }
    Ok(n.trunc() as usize)
}

fn offscreen_ctor(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    if ctx.new_target.is_undefined() {
        return vm.throw_type("Failed to construct 'OffscreenCanvas': Please use the 'new' operator, this DOM object constructor cannot be called as a function.");
    }
    need(vm, ctx, 2, "OffscreenCanvas", "constructor")?;
    let w = enforce_ull(vm, &arg(vm, ctx, 0), "construct 'OffscreenCanvas'")?;
    let h = enforce_ull(vm, &arg(vm, ctx, 1), "construct 'OffscreenCanvas'")?;
    let default = iface("OffscreenCanvas").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    let proto = proto_from_new_target(vm, &ctx.new_target, default)?;
    Ok(Value::Object(host_obj(vm, proto, T_OFF, vec![num(-1.0), num(w as f64), num(h as f64), Value::Undefined])))
}

fn offscreen_dim_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_OFF) else { return throw_illegal(vm) };
    Ok(slot(vm, o, if callee_str(vm, ctx) == "width" { 1 } else { 2 }))
}

fn offscreen_dim_set(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_OFF) else { return throw_illegal(vm) };
    let which = callee_str(vm, ctx);
    let v = enforce_ull(vm, &arg(vm, ctx, 0), "set the OffscreenCanvas size")?;
    set_slot(vm, o, if which == "width" { 1 } else { 2 }, num(v as f64));
    let sid = slot_num(vm, o, 0);
    if sid >= 0.0 {
        let (w, h) = (slot_num(vm, o, 1) as usize, slot_num(vm, o, 2) as usize);
        canvas::with(sid as u32, |c| c.resize(w, h));
        if let Some(c) = slot(vm, o, 3).as_object() {
            refresh_keepalive(vm, c, sid as u32);
        }
    }
    Ok(Value::Undefined)
}

fn offscreen_get_context(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_OFF) else { return throw_illegal(vm) };
    need(vm, ctx, 1, "OffscreenCanvas", "getContext")?;
    let id = string(vm, &arg(vm, ctx, 0))?;
    match id.as_str() {
        "2d" => {}
        "webgl" | "webgl2" | "webgpu" | "bitmaprenderer" => {
            crate::ledger::record_dom(&format!("offscreen-context-refused:{id}"));
            return Ok(Value::Null);
        }
        _ => {
            return vm.throw_type(&format!("Failed to execute 'getContext' on 'OffscreenCanvas': The provided value '{id}' is not a valid enum value of type OffscreenRenderingContextId."));
        }
    }
    let existing = slot(vm, o, 3);
    if existing.is_object() {
        return Ok(existing);
    }
    let st = context_settings(vm, &arg(vm, ctx, 1))?;
    let (w, h) = (slot_num(vm, o, 1) as usize, slot_num(vm, o, 2) as usize);
    let sid = canvas::create(w, h, !st.alpha);
    canvas::with(sid, |c| {
        c.will_read = st.will_read;
        c.color_space = st.color_space;
    });
    let c = new_context(vm, "OffscreenCanvasRenderingContext2D", sid, Value::Object(o), 1.0);
    set_slot(vm, o, 0, num(sid as f64));
    set_slot(vm, o, 3, Value::Object(c));
    Ok(Value::Object(c))
}

fn offscreen_png(vm: &Vm, o: Obj) -> Option<Vec<u8>> {
    let (w, h) = (slot_num(vm, o, 1) as usize, slot_num(vm, o, 2) as usize);
    if w == 0 || h == 0 || w.saturating_mul(h) > canvas::context::MAX_PIXELS {
        return None;
    }
    let sid = slot_num(vm, o, 0);
    if sid >= 0.0 {
        if let Some(p) = canvas::with(sid as u32, |c| c.to_png()).flatten() {
            return Some(p);
        }
    }
    pixel_core::png::encode::encode_rgba(w as u32, h as u32, &vec![0u8; w * h * 4]).ok()
}

fn new_bitmap(vm: &mut Vm, b: &Bitmap) -> JsResult<Value> {
    let data = new_hidden_buffer(vm, (*b.rgba).clone())?;
    let proto = iface("ImageBitmap").map(|i| i.proto).unwrap_or_else(|| vm.intr().object_proto);
    Ok(Value::Object(host_obj(vm, proto, T_BMP, vec![num(b.w as f64), num(b.h as f64), Value::Object(data)])))
}

fn offscreen_transfer(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_OFF) else { return throw_illegal(vm) };
    let (w, h) = (slot_num(vm, o, 1) as usize, slot_num(vm, o, 2) as usize);
    let sid = slot_num(vm, o, 0);
    if sid < 0.0 {
        return throw_dom(vm, "InvalidStateError", "Failed to execute 'transferToImageBitmap' on 'OffscreenCanvas': Cannot transfer an ImageBitmap from an OffscreenCanvas with no context");
    }
    if w == 0 || h == 0 {
        return throw_dom(vm, "InvalidStateError", "Failed to execute 'transferToImageBitmap' on 'OffscreenCanvas': Cannot transfer an ImageBitmap from a zero-sized OffscreenCanvas");
    }
    let snap = canvas::with(sid as u32, |c| {
        let s = c.snapshot();
        // The bitmap is replaced by a fresh transparent one; the drawing state is kept.
        let opaque = c.opaque;
        for p in c.px.chunks_exact_mut(4) {
            p.copy_from_slice(&[0, 0, 0, if opaque { 255 } else { 0 }]);
        }
        s
    });
    match snap {
        Some(b) => new_bitmap(vm, &b),
        None => Ok(Value::Null),
    }
}

fn bitmap_get(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_BMP) else { return throw_illegal(vm) };
    if slot(vm, o, 2).is_undefined() {
        return Ok(num(0.0));
    }
    Ok(slot(vm, o, if callee_str(vm, ctx) == "width" { 0 } else { 1 }))
}

fn bitmap_close(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let Some(o) = this_tagged(vm, &ctx.this, T_BMP) else { return throw_illegal(vm) };
    set_slot(vm, o, 2, Value::Undefined);
    Ok(Value::Undefined)
}

/// The synchronous core of `createImageBitmap(image, [sx, sy, sw, sh,] options)` (the prelude wraps it in a
/// promise; a Blob arrives as its bytes, a Uint8Array to decode).
fn make_bitmap_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    const I: &str = "Window";
    const M: &str = "createImageBitmap";
    need(vm, ctx, 1, I, M)?;
    let src = arg(vm, ctx, 0);
    let img: Bitmap = if let Some(o) = src.as_object().filter(|o| tag_of(vm, *o) == Some(T_IDATA)) {
        let (w, h) = (slot_num(vm, o, 0) as usize, slot_num(vm, o, 1) as usize);
        let bytes = super::net::buffer_bytes(vm, &slot(vm, o, 2)).unwrap_or_default();
        if bytes.is_empty() {
            return throw_dom(vm, "InvalidStateError", "Failed to execute 'createImageBitmap' on 'Window': The image data has been detached.");
        }
        Bitmap { w, h, rgba: Rc::new(bytes) }
    } else if src.as_object().is_some_and(|o| matches!(vm.heap.get(o).kind, Kind::TypedArray(_))) {
        let bytes = super::net::buffer_bytes(vm, &src).unwrap_or_default();
        match crate::images::decode_raster(&bytes) {
            Some(i) => Bitmap { w: i.width() as usize, h: i.height() as usize, rgba: Rc::new(i.into_raw()) },
            None => return throw_dom(vm, "InvalidStateError", "Failed to execute 'createImageBitmap' on 'Window': The source image could not be decoded."),
        }
    } else {
        match image_source(vm, &src, I, M)? {
            Some(b) => b,
            None => return throw_dom(vm, "InvalidStateError", "Failed to execute 'createImageBitmap' on 'Window': The image element contains no image data."),
        }
    };
    let (mut sx, mut sy, mut sw, mut sh) = (0i64, 0i64, img.w as i64, img.h as i64);
    let mut opt_index = 1;
    if ctx.argc >= 5 {
        let mut v = [0i64; 4];
        for (k, slot_v) in v.iter_mut().enumerate() {
            *slot_v = enforce_long(vm, &arg(vm, ctx, 1 + k), I, M)?;
        }
        [sx, sy, sw, sh] = v;
        if sw == 0 || sh == 0 {
            return vm.throw_range(&format!("Failed to execute 'createImageBitmap' on 'Window': The crop rect {} is 0.", if sw == 0 { "width" } else { "height" }));
        }
        if sw < 0 {
            sx += sw;
            sw = -sw;
        }
        if sh < 0 {
            sy += sh;
            sh = -sh;
        }
        opt_index = 5;
    } else if ctx.argc > 2 {
        return vm.throw_type("Failed to execute 'createImageBitmap' on 'Window': Valid arities are: [1, 2, 5, 6].");
    }
    let opts = arg(vm, ctx, opt_index);
    let flip = {
        let v = dict_member(vm, &opts, "imageOrientation")?;
        !v.is_undefined() && string(vm, &v)? == "flipY"
    };
    let rw = dict_member(vm, &opts, "resizeWidth")?;
    let rh = dict_member(vm, &opts, "resizeHeight")?;
    let rw = if rw.is_undefined() { None } else { Some(enforce_ull(vm, &rw, "execute 'createImageBitmap'")?) };
    let rh = if rh.is_undefined() { None } else { Some(enforce_ull(vm, &rh, "execute 'createImageBitmap'")?) };
    if rw == Some(0) || rh == Some(0) {
        return throw_dom(vm, "InvalidStateError", "Failed to execute 'createImageBitmap' on 'Window': The resize width or height is 0.");
    }
    // Crop (outside the source: transparent black).
    let (cw, ch) = (sw as usize, sh as usize);
    if cw.saturating_mul(ch) > canvas::context::MAX_PIXELS {
        return vm.throw_range("Failed to execute 'createImageBitmap' on 'Window': The crop rect is too large.");
    }
    let mut out = vec![0u8; cw * ch * 4];
    for y in 0..ch {
        let iy = sy + y as i64;
        if iy < 0 || iy >= img.h as i64 {
            continue;
        }
        for x in 0..cw {
            let ix = sx + x as i64;
            if ix < 0 || ix >= img.w as i64 {
                continue;
            }
            let s0 = ((iy as usize) * img.w + ix as usize) * 4;
            let d0 = (y * cw + x) * 4;
            out[d0..d0 + 4].copy_from_slice(&img.rgba[s0..s0 + 4]);
        }
    }
    let (mut ow, mut oh) = (cw, ch);
    match (rw, rh) {
        (Some(w), Some(h)) => {
            ow = w;
            oh = h;
        }
        (Some(w), None) => {
            ow = w;
            oh = ((w as f64) * ch as f64 / cw as f64).ceil().max(1.0) as usize;
        }
        (None, Some(h)) => {
            oh = h;
            ow = ((h as f64) * cw as f64 / ch as f64).ceil().max(1.0) as usize;
        }
        _ => {}
    }
    if (ow, oh) != (cw, ch) {
        if ow.saturating_mul(oh) > canvas::context::MAX_PIXELS {
            return vm.throw_range("Failed to execute 'createImageBitmap' on 'Window': The resize size is too large.");
        }
        let src_b = Bitmap { w: cw, h: ch, rgba: Rc::new(out) };
        let mut r = vec![0u8; ow * oh * 4];
        for y in 0..oh {
            for x in 0..ow {
                let u = (x as f64 + 0.5) * cw as f64 / ow as f64;
                let v = (y as f64 + 0.5) * ch as f64 / oh as f64;
                let c = canvas::paint::sample_image(&src_b, Repeat::None, true, (0.0, 0.0, cw as f64, ch as f64), u, v);
                let d0 = (y * ow + x) * 4;
                let a = c[3];
                if a > 0.0 {
                    r[d0] = (c[0] / a * 255.0 + 0.5).min(255.0) as u8;
                    r[d0 + 1] = (c[1] / a * 255.0 + 0.5).min(255.0) as u8;
                    r[d0 + 2] = (c[2] / a * 255.0 + 0.5).min(255.0) as u8;
                    r[d0 + 3] = (a * 255.0 + 0.5) as u8;
                }
            }
        }
        out = r;
    }
    if flip {
        let row = ow * 4;
        for y in 0..oh / 2 {
            let (a, b) = (y * row, (oh - 1 - y) * row);
            for k in 0..row {
                out.swap(a + k, b + k);
            }
        }
    }
    new_bitmap(vm, &Bitmap { w: ow, h: oh, rgba: Rc::new(out) })
}

// ------------------------------------------------------------------------------------------------ install

const PATH_METHODS: [(&str, u32); 10] = [
    ("closePath", 0),
    ("moveTo", 2),
    ("lineTo", 2),
    ("quadraticCurveTo", 4),
    ("bezierCurveTo", 6),
    ("arcTo", 5),
    ("rect", 4),
    ("roundRect", 4),
    ("arc", 5),
    ("ellipse", 7),
];

const CTX_METHODS: [(&str, u32); 36] = [
    ("save", 0),
    ("restore", 0),
    ("reset", 0),
    ("isContextLost", 0),
    ("scale", 2),
    ("rotate", 1),
    ("translate", 2),
    ("transform", 6),
    ("getTransform", 0),
    ("setTransform", 0),
    ("resetTransform", 0),
    ("createLinearGradient", 4),
    ("createRadialGradient", 6),
    ("createConicGradient", 3),
    ("createPattern", 2),
    ("clearRect", 4),
    ("fillRect", 4),
    ("strokeRect", 4),
    ("beginPath", 0),
    ("fill", 0),
    ("stroke", 0),
    ("clip", 0),
    ("isPointInPath", 2),
    ("isPointInStroke", 2),
    ("drawFocusIfNeeded", 1),
    ("scrollPathIntoView", 0),
    ("fillText", 3),
    ("strokeText", 3),
    ("measureText", 1),
    ("drawImage", 3),
    ("createImageData", 1),
    ("getImageData", 4),
    ("putImageData", 3),
    ("setLineDash", 1),
    ("getLineDash", 0),
    ("getContextAttributes", 0),
];

const CTX_ATTRS: [&str; 26] = [
    "globalAlpha",
    "globalCompositeOperation",
    "imageSmoothingEnabled",
    "imageSmoothingQuality",
    "strokeStyle",
    "fillStyle",
    "shadowOffsetX",
    "shadowOffsetY",
    "shadowBlur",
    "shadowColor",
    "filter",
    "lineWidth",
    "lineCap",
    "lineJoin",
    "miterLimit",
    "lineDashOffset",
    "font",
    "textAlign",
    "textBaseline",
    "direction",
    "letterSpacing",
    "fontKerning",
    "fontStretch",
    "fontVariantCaps",
    "textRendering",
    "wordSpacing",
];

fn install_context_proto(vm: &mut Vm, p: Obj) {
    attr_with(vm, p, "canvas", ctx_attr_get, None, s("canvas"));
    for (n, len) in CTX_METHODS {
        op_with(vm, p, n, len, ctx_method, s(n));
    }
    for (n, len) in PATH_METHODS {
        op_with(vm, p, n, len, path_method, s(n));
    }
    for n in CTX_ATTRS {
        attr_with(vm, p, n, ctx_attr_get, Some(ctx_attr_set), s(n));
    }
}

pub fn install(vm: &mut Vm) {
    canvas::reset();
    DOMMATRIX.with(|d| d.set(None));

    let c2d = interface(vm, "CanvasRenderingContext2D", None, None);
    register_iface("CanvasRenderingContext2D", c2d);
    install_context_proto(vm, c2d.proto);
    let o2d = interface(vm, "OffscreenCanvasRenderingContext2D", None, None);
    register_iface("OffscreenCanvasRenderingContext2D", o2d);
    install_context_proto(vm, o2d.proto);

    let p2d = interface(vm, "Path2D", None, Some((path2d_ctor, 0)));
    register_iface("Path2D", p2d);
    op(vm, p2d.proto, "addPath", 1, path2d_add_path);
    for (n, len) in PATH_METHODS {
        op_with(vm, p2d.proto, n, len, path_method, s(n));
    }

    let grad = interface(vm, "CanvasGradient", None, None);
    register_iface("CanvasGradient", grad);
    op(vm, grad.proto, "addColorStop", 2, gradient_add_stop);
    let pat = interface(vm, "CanvasPattern", None, None);
    register_iface("CanvasPattern", pat);
    op(vm, pat.proto, "setTransform", 0, pattern_set_transform);

    let tm = interface(vm, "TextMetrics", None, None);
    register_iface("TextMetrics", tm);
    for (i, n) in [
        "width",
        "actualBoundingBoxLeft",
        "actualBoundingBoxRight",
        "fontBoundingBoxAscent",
        "fontBoundingBoxDescent",
        "actualBoundingBoxAscent",
        "actualBoundingBoxDescent",
        "emHeightAscent",
        "emHeightDescent",
        "hangingBaseline",
        "alphabeticBaseline",
        "ideographicBaseline",
    ]
    .iter()
    .enumerate()
    {
        attr_with(vm, tm.proto, n, metrics_get, None, s(&i.to_string()));
    }

    let idata = interface(vm, "ImageData", None, Some((image_data_ctor, 2)));
    register_iface("ImageData", idata);
    for (i, n) in ["width", "height", "data", "colorSpace"].iter().enumerate() {
        attr_with(vm, idata.proto, n, image_data_get, None, s(&i.to_string()));
    }

    let et = iface("EventTarget");
    let off = interface(vm, "OffscreenCanvas", et, Some((offscreen_ctor, 2)));
    register_iface("OffscreenCanvas", off);
    attr_with(vm, off.proto, "width", offscreen_dim_get, Some(offscreen_dim_set), s("width"));
    attr_with(vm, off.proto, "height", offscreen_dim_get, Some(offscreen_dim_set), s("height"));
    op(vm, off.proto, "getContext", 1, offscreen_get_context);
    op(vm, off.proto, "transferToImageBitmap", 0, offscreen_transfer);

    let bmp = interface(vm, "ImageBitmap", None, None);
    register_iface("ImageBitmap", bmp);
    attr_with(vm, bmp.proto, "width", bitmap_get, None, s("width"));
    attr_with(vm, bmp.proto, "height", bitmap_get, None, s("height"));
    op(vm, bmp.proto, "close", 0, bitmap_close);

    // HTMLCanvasElement: replaces html.rs's stubs (getContext → null, toDataURL → "data:,") and the
    // reflected width/height (their setters must also reset the bitmap).
    if let Some(hc) = iface("HTMLCanvasElement") {
        op(vm, hc.proto, "getContext", 1, canvas_get_context);
        op(vm, hc.proto, "toDataURL", 0, canvas_to_data_url);
        attr_with(vm, hc.proto, "width", canvas_dim_get, Some(canvas_dim_set), s("width"));
        attr_with(vm, hc.proto, "height", canvas_dim_get, Some(canvas_dim_set), s("height"));
    }

    // The prelude: DOMMatrix, toBlob, convertToBlob, createImageBitmap.
    let g = vm.realm().global;
    for (name, len, f) in [("__canvas_png", 1, canvas_png_native as NativeFn), ("__canvas_bitmap", 1, make_bitmap_native)] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("canvas-prelude-failed:{}", super::clip(&m, 64)));
    }
    if let Ok(Value::Object(c)) = vm.get(g, &PropertyKey::from_str("DOMMatrix")) {
        let r = root(vm, Value::Object(c));
        DOMMATRIX.with(|d| d.set(Some(r)));
    }
}

/// DOMMatrixReadOnly / DOMMatrix (Geometry Interfaces §6, the matrix as 16 doubles + is2D), and the
/// promise-returning canvas entry points over the natives above.
const PRELUDE: &str = r##"
(function () {
    var png = globalThis.__canvas_png, mkBitmap = globalThis.__canvas_bitmap;
    delete globalThis.__canvas_png;
    delete globalThis.__canvas_bitmap;
    var hide = function (o, name, v) {
        Object.defineProperty(o, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var method = function (proto, name, f) {
        Object.defineProperty(proto, name, { value: f, writable: true, enumerable: true, configurable: true });
    };

    if (typeof DOMMatrixReadOnly === 'undefined') {
        var ST = new WeakMap();
        var st = function (o) {
            var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
            if (s === undefined) { throw new TypeError('Illegal invocation'); }
            return s;
        };
        // m = [m11, m12, m13, m14, m21, m22, m23, m24, m31, m32, m33, m34, m41, m42, m43, m44]
        var ident = function () { return [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]; };
        var mul = function (a, b) { // a × b (b applied first)
            var o = new Array(16);
            for (var c = 0; c < 4; c++) {
                for (var r = 0; r < 4; r++) {
                    var v = 0;
                    for (var k = 0; k < 4; k++) { v += a[k * 4 + r] * b[c * 4 + k]; }
                    o[c * 4 + r] = v;
                }
            }
            return o;
        };
        var from2d = function (a, b, c, d, e, f) { return [a, b, 0, 0, c, d, 0, 0, 0, 0, 1, 0, e, f, 0, 1]; };
        var parseInit = function (init, ctorName) {
            if (init === undefined) { return { m: ident(), is2D: true }; }
            if (typeof init === 'string') {
                var t = init.trim();
                if (t === '' || t === 'none') { return { m: ident(), is2D: true }; }
                var mm = /^matrix(3d)?\(([^)]*)\)$/.exec(t);
                if (mm) {
                    var ns = mm[2].split(/\s*,\s*/).map(Number);
                    if (!mm[1] && ns.length === 6 && ns.every(isFinite)) { return { m: from2d.apply(null, ns), is2D: true }; }
                    if (mm[1] && ns.length === 16 && ns.every(isFinite)) { return { m: ns, is2D: false }; }
                }
                throw new DOMException("Failed to construct '" + ctorName + "': Failed to parse '" + init + "'.", 'SyntaxError');
            }
            var arr = Array.from(init, Number);
            if (arr.length === 6) { return { m: from2d.apply(null, arr), is2D: true }; }
            if (arr.length === 16) { return { m: arr, is2D: false }; }
            throw new TypeError("Failed to construct '" + ctorName + "': The sequence must contain 6 elements for a 2D matrix or 16 elements for a 3D matrix.");
        };
        var fromDict = function (d) {
            if (d === undefined || d === null) { return { m: ident(), is2D: true }; }
            if (ST.has(d)) { var s0 = ST.get(d); return { m: s0.m.slice(), is2D: s0.is2D }; }
            var pick = function (short, long, def) {
                var a = d[short], b = d[long];
                if (a !== undefined && b !== undefined && !(Object.is(+a, +b) || (+a === +b))) { throw new TypeError('Property mismatch on matrix initialization.'); }
                return b !== undefined ? +b : (a !== undefined ? +a : def);
            };
            var m = from2d(pick('a', 'm11', 1), pick('b', 'm12', 0), pick('c', 'm21', 0), pick('d', 'm22', 1), pick('e', 'm41', 0), pick('f', 'm42', 0));
            var is3d = false;
            [['m13', 2, 0], ['m14', 3, 0], ['m23', 6, 0], ['m24', 7, 0], ['m31', 8, 0], ['m32', 9, 0], ['m33', 10, 1], ['m34', 11, 0], ['m43', 14, 0], ['m44', 15, 1]].forEach(function (e) {
                if (d[e[0]] !== undefined) { m[e[1]] = +d[e[0]]; if (m[e[1]] !== e[2]) { is3d = true; } }
            });
            if (d.is2D === true && is3d) { throw new TypeError('The is2D member is set to true but the input matrix is a 3d matrix.'); }
            return { m: m, is2D: d.is2D === false ? false : !is3d };
        };
        var invert = function (m) {
            var inv = new Array(16), det;
            inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15] + m[9] * m[7] * m[14] + m[13] * m[6] * m[11] - m[13] * m[7] * m[10];
            inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15] - m[8] * m[7] * m[14] - m[12] * m[6] * m[11] + m[12] * m[7] * m[10];
            inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15] + m[8] * m[7] * m[13] + m[12] * m[5] * m[11] - m[12] * m[7] * m[9];
            inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14] - m[8] * m[6] * m[13] - m[12] * m[5] * m[10] + m[12] * m[6] * m[9];
            inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15] - m[9] * m[3] * m[14] - m[13] * m[2] * m[11] + m[13] * m[3] * m[10];
            inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15] + m[8] * m[3] * m[14] + m[12] * m[2] * m[11] - m[12] * m[3] * m[10];
            inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15] - m[8] * m[3] * m[13] - m[12] * m[1] * m[11] + m[12] * m[3] * m[9];
            inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14] + m[8] * m[2] * m[13] + m[12] * m[1] * m[10] - m[12] * m[2] * m[9];
            inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15] + m[5] * m[3] * m[14] + m[13] * m[2] * m[7] - m[13] * m[3] * m[6];
            inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15] - m[4] * m[3] * m[14] - m[12] * m[2] * m[7] + m[12] * m[3] * m[6];
            inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15] + m[4] * m[3] * m[13] + m[12] * m[1] * m[7] - m[12] * m[3] * m[5];
            inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14] - m[4] * m[2] * m[13] - m[12] * m[1] * m[6] + m[12] * m[2] * m[5];
            inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11] - m[5] * m[3] * m[10] - m[9] * m[2] * m[7] + m[9] * m[3] * m[6];
            inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11] + m[4] * m[3] * m[10] + m[8] * m[2] * m[7] - m[8] * m[3] * m[6];
            inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11] - m[4] * m[3] * m[9] - m[8] * m[1] * m[7] + m[8] * m[3] * m[5];
            inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10] + m[4] * m[2] * m[9] + m[8] * m[1] * m[6] - m[8] * m[2] * m[5];
            det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
            if (det === 0 || !isFinite(det)) { return null; }
            for (var i = 0; i < 16; i++) { inv[i] /= det; }
            return inv;
        };
        var rad = function (deg) { return deg * Math.PI / 180; };
        var rotAxis = function (x, y, z, deg) {
            var len = Math.sqrt(x * x + y * y + z * z);
            if (len === 0) { return ident(); }
            x /= len; y /= len; z /= len;
            var a = rad(deg), s = Math.sin(a), c = Math.cos(a), t = 1 - c;
            return [t * x * x + c, t * x * y + s * z, t * x * z - s * y, 0,
                    t * x * y - s * z, t * y * y + c, t * y * z + s * x, 0,
                    t * x * z + s * y, t * y * z - s * x, t * z * z + c, 0,
                    0, 0, 0, 1];
        };
        var NAMES = ['m11', 'm12', 'm13', 'm14', 'm21', 'm22', 'm23', 'm24', 'm31', 'm32', 'm33', 'm34', 'm41', 'm42', 'm43', 'm44'];
        var ALIAS = { a: 0, b: 1, c: 4, d: 5, e: 12, f: 13 };
        var make = function (proto, s) { var o = Object.create(proto); ST.set(o, s); return o; };

        class DOMMatrixReadOnly {
            constructor(init) { ST.set(this, parseInit(init, new.target === DOMMatrix ? 'DOMMatrix' : 'DOMMatrixReadOnly')); }
            static fromMatrix(other) { return make(DOMMatrixReadOnly.prototype, fromDict(other)); }
            static fromFloat32Array(a) { return new DOMMatrixReadOnly(Array.from(a)); }
            static fromFloat64Array(a) { return new DOMMatrixReadOnly(Array.from(a)); }
            get is2D() { return st(this).is2D; }
            get isIdentity() { var m = st(this).m, i = ident(); for (var k = 0; k < 16; k++) { if (m[k] !== i[k]) { return false; } } return true; }
            translate(tx, ty, tz) { var r = new DOMMatrix(); var s = st(r), t = st(this); s.m = t.m.slice(); s.is2D = t.is2D; return r.translateSelf(tx, ty, tz); }
            scale(sx, sy, sz, ox, oy, oz) { var r = DOMMatrix.fromMatrix(this); return r.scaleSelf(sx, sy, sz, ox, oy, oz); }
            scale3d(s, ox, oy, oz) { return DOMMatrix.fromMatrix(this).scale3dSelf(s, ox, oy, oz); }
            scaleNonUniform(sx, sy) { return DOMMatrix.fromMatrix(this).scaleSelf(sx, sy === undefined ? 1 : sy, 1); }
            rotate(rx, ry, rz) { return DOMMatrix.fromMatrix(this).rotateSelf(rx, ry, rz); }
            rotateFromVector(x, y) { return DOMMatrix.fromMatrix(this).rotateFromVectorSelf(x, y); }
            rotateAxisAngle(x, y, z, a) { return DOMMatrix.fromMatrix(this).rotateAxisAngleSelf(x, y, z, a); }
            skewX(a) { return DOMMatrix.fromMatrix(this).skewXSelf(a); }
            skewY(a) { return DOMMatrix.fromMatrix(this).skewYSelf(a); }
            multiply(other) { return DOMMatrix.fromMatrix(this).multiplySelf(other); }
            flipX() { return DOMMatrix.fromMatrix(this).multiplySelf({ a: -1 }); }
            flipY() { return DOMMatrix.fromMatrix(this).multiplySelf({ d: -1 }); }
            inverse() { return DOMMatrix.fromMatrix(this).invertSelf(); }
            transformPoint(p) {
                p = p || {};
                var x = p.x === undefined ? 0 : +p.x, y = p.y === undefined ? 0 : +p.y, z = p.z === undefined ? 0 : +p.z, w = p.w === undefined ? 1 : +p.w;
                var m = st(this).m;
                var o = { x: m[0] * x + m[4] * y + m[8] * z + m[12] * w, y: m[1] * x + m[5] * y + m[9] * z + m[13] * w,
                          z: m[2] * x + m[6] * y + m[10] * z + m[14] * w, w: m[3] * x + m[7] * y + m[11] * z + m[15] * w };
                return typeof DOMPoint === 'function' ? new DOMPoint(o.x, o.y, o.z, o.w) : o;
            }
            toFloat32Array() { return new Float32Array(st(this).m); }
            toFloat64Array() { return new Float64Array(st(this).m); }
            toJSON() { var o = {}, self = this; ['a', 'b', 'c', 'd', 'e', 'f'].concat(NAMES).forEach(function (n) { o[n] = self[n]; }); o.is2D = this.is2D; o.isIdentity = this.isIdentity; return o; }
            toString() {
                var s = st(this), m = s.m;
                if (!m.every(isFinite)) { throw new DOMException('Cannot be serialized with NaN or Infinity values.', 'InvalidStateError'); }
                return s.is2D ? 'matrix(' + [m[0], m[1], m[4], m[5], m[12], m[13]].join(', ') + ')' : 'matrix3d(' + m.join(', ') + ')';
            }
        }
        NAMES.forEach(function (n, i) {
            Object.defineProperty(DOMMatrixReadOnly.prototype, n, { get: function () { return st(this).m[i]; }, enumerable: true, configurable: true });
        });
        Object.keys(ALIAS).forEach(function (n) {
            var i = ALIAS[n];
            Object.defineProperty(DOMMatrixReadOnly.prototype, n, { get: function () { return st(this).m[i]; }, enumerable: true, configurable: true });
        });
        Object.defineProperty(DOMMatrixReadOnly.prototype, Symbol.toStringTag, { value: 'DOMMatrixReadOnly', configurable: true });

        class DOMMatrix extends DOMMatrixReadOnly {
            constructor(init) { super(init); }
            static fromMatrix(other) { return make(DOMMatrix.prototype, fromDict(other)); }
            static fromFloat32Array(a) { return new DOMMatrix(Array.from(a)); }
            static fromFloat64Array(a) { return new DOMMatrix(Array.from(a)); }
            multiplySelf(other) { var s = st(this), o = fromDict(other); s.m = mul(s.m, o.m); s.is2D = s.is2D && o.is2D; return this; }
            preMultiplySelf(other) { var s = st(this), o = fromDict(other); s.m = mul(o.m, s.m); s.is2D = s.is2D && o.is2D; return this; }
            translateSelf(tx, ty, tz) {
                tx = tx === undefined ? 0 : +tx; ty = ty === undefined ? 0 : +ty; tz = tz === undefined ? 0 : +tz;
                var s = st(this); s.m = mul(s.m, [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, tx, ty, tz, 1]); if (tz !== 0) { s.is2D = false; } return this;
            }
            scaleSelf(sx, sy, sz, ox, oy, oz) {
                sx = sx === undefined ? 1 : +sx; sy = sy === undefined ? sx : +sy; sz = sz === undefined ? 1 : +sz;
                ox = ox === undefined ? 0 : +ox; oy = oy === undefined ? 0 : +oy; oz = oz === undefined ? 0 : +oz;
                this.translateSelf(ox, oy, oz);
                var s = st(this); s.m = mul(s.m, [sx, 0, 0, 0, 0, sy, 0, 0, 0, 0, sz, 0, 0, 0, 0, 1]);
                if (sz !== 1 || oz !== 0) { s.is2D = false; }
                return this.translateSelf(-ox, -oy, -oz);
            }
            scale3dSelf(sc, ox, oy, oz) { sc = sc === undefined ? 1 : +sc; return this.scaleSelf(sc, sc, sc, ox, oy, oz); }
            rotateSelf(rx, ry, rz) {
                rx = rx === undefined ? 0 : +rx;
                if (ry === undefined && rz === undefined) { rz = rx; rx = 0; ry = 0; }
                ry = ry === undefined ? 0 : +ry; rz = rz === undefined ? 0 : +rz;
                var s = st(this);
                if (rz) { s.m = mul(s.m, rotAxis(0, 0, 1, rz)); }
                if (ry) { s.m = mul(s.m, rotAxis(0, 1, 0, ry)); s.is2D = false; }
                if (rx) { s.m = mul(s.m, rotAxis(1, 0, 0, rx)); s.is2D = false; }
                return this;
            }
            rotateFromVectorSelf(x, y) { x = x === undefined ? 0 : +x; y = y === undefined ? 0 : +y; var a = (x === 0 && y === 0) ? 0 : Math.atan2(y, x) * 180 / Math.PI; return this.rotateSelf(0, 0, a); }
            rotateAxisAngleSelf(x, y, z, a) {
                x = x === undefined ? 0 : +x; y = y === undefined ? 0 : +y; z = z === undefined ? 0 : +z; a = a === undefined ? 0 : +a;
                var s = st(this); s.m = mul(s.m, rotAxis(x, y, z, a)); if (x !== 0 || y !== 0) { s.is2D = false; } return this;
            }
            skewXSelf(a) { a = a === undefined ? 0 : +a; var s = st(this); s.m = mul(s.m, from2d(1, 0, Math.tan(rad(a)), 1, 0, 0)); return this; }
            skewYSelf(a) { a = a === undefined ? 0 : +a; var s = st(this); s.m = mul(s.m, from2d(1, Math.tan(rad(a)), 0, 1, 0, 0)); return this; }
            invertSelf() { var s = st(this), r = invert(s.m); if (r === null) { s.m = s.m.map(function () { return NaN; }); s.is2D = false; } else { s.m = r; } return this; }
            setMatrixValue(t) { var p = parseInit(String(t), 'DOMMatrix'); var s = st(this); s.m = p.m; s.is2D = p.is2D; return this; }
        }
        NAMES.forEach(function (n, i) {
            Object.defineProperty(DOMMatrix.prototype, n, {
                get: function () { return st(this).m[i]; },
                set: function (v) { var s = st(this); s.m[i] = +v; if ([2, 3, 6, 7, 8, 9, 11, 14].indexOf(i) >= 0 && +v !== 0) { s.is2D = false; } if ((i === 10 || i === 15) && +v !== 1) { s.is2D = false; } },
                enumerable: true, configurable: true
            });
        });
        Object.keys(ALIAS).forEach(function (n) {
            var i = ALIAS[n];
            Object.defineProperty(DOMMatrix.prototype, n, { get: function () { return st(this).m[i]; }, set: function (v) { st(this).m[i] = +v; }, enumerable: true, configurable: true });
        });
        Object.defineProperty(DOMMatrix.prototype, Symbol.toStringTag, { value: 'DOMMatrix', configurable: true });
        hide(globalThis, 'DOMMatrixReadOnly', DOMMatrixReadOnly);
        hide(globalThis, 'DOMMatrix', DOMMatrix);
        if (typeof WebKitCSSMatrix === 'undefined') { hide(globalThis, 'WebKitCSSMatrix', DOMMatrix); }
    }

    var blobOf = function (bytes) { return bytes === null ? null : new Blob([bytes], { type: 'image/png' }); };
    if (typeof HTMLCanvasElement === 'function') {
        method(HTMLCanvasElement.prototype, 'toBlob', function toBlob(callback) {
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'toBlob' on 'HTMLCanvasElement': 1 argument required, but only 0 present."); }
            if (typeof callback !== 'function') { throw new TypeError("Failed to execute 'toBlob' on 'HTMLCanvasElement': The callback provided as parameter 1 is not a function."); }
            var bytes = png(this);
            setTimeout(function () { callback(blobOf(bytes)); }, 0);
        });
    }
    if (typeof OffscreenCanvas === 'function') {
        method(OffscreenCanvas.prototype, 'convertToBlob', function convertToBlob(options) {
            var self = this;
            return new Promise(function (resolve, reject) {
                var bytes = png(self);
                if (bytes === null) { reject(new DOMException("Failed to execute 'convertToBlob' on 'OffscreenCanvas': The canvas has no pixels.", 'IndexSizeError')); return; }
                resolve(blobOf(bytes));
            });
        });
    }
    hide(globalThis, 'createImageBitmap', function createImageBitmap(image) {
        var args = Array.prototype.slice.call(arguments);
        return new Promise(function (resolve) {
            if (typeof Blob === 'function' && image instanceof Blob && typeof __blob_bytes === 'function') { args[0] = __blob_bytes(image); }
            resolve(mkBitmap.apply(undefined, args));
        });
    });
})();
"##;
