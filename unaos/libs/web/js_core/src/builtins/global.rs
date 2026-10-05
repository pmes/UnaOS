//! Function properties of the global object (§19.2): eval, isFinite, isNaN, parseFloat, parseInt, the URI
//! functions, and Annex B escape / unescape.

use super::*;
use crate::numconv;

pub fn init(vm: &mut Vm) {
    let g = vm.realm().global;
    let ev = method(vm, g, "eval", 1, eval);
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.eval_fn = ev;
    method(vm, g, "isFinite", 1, is_finite);
    method(vm, g, "isNaN", 1, is_nan);
    let pf = method(vm, g, "parseFloat", 1, parse_float);
    let pi = method(vm, g, "parseInt", 2, parse_int);
    method(vm, g, "decodeURI", 1, decode_uri);
    method(vm, g, "decodeURIComponent", 1, decode_uri_component);
    method(vm, g, "encodeURI", 1, encode_uri);
    method(vm, g, "encodeURIComponent", 1, encode_uri_component);
    method(vm, g, "escape", 1, escape);
    method(vm, g, "unescape", 1, unescape);
    let nc = vm.realms[r].intrinsics.number_ctor;
    vm.heap.get_mut(nc).props.insert(PropertyKey::from_str("parseFloat"), Prop::data(Value::Object(pf), WC));
    vm.heap.get_mut(nc).props.insert(PropertyKey::from_str("parseInt"), Prop::data(Value::Object(pi), WC));
}

/// Indirect eval (§19.2.1): global scope of this function's realm.
fn eval(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let x = vm.arg(ctx, 0);
    let src = match x {
        Value::String(s) => s,
        other => return Ok(other),
    };
    let ec = crate::parser::EvalContext::default();
    let prog = match crate::parser::parse_eval(src.units(), &ec) {
        Ok(p) => p,
        Err(e) => return vm.throw_syntax(&e.msg),
    };
    let code = crate::compiler::compile_eval(&prog);
    let r = vm.cur_realm;
    let genv = vm.realms[r as usize].global_env;
    let gt = vm.realms[r as usize].global_this.clone();
    vm.run_eval_code(code, Some(genv), gt, Value::Undefined, None, r, None, prog.strict)
}

fn is_finite(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    Ok(Value::Bool(vm.to_number(&a)?.is_finite()))
}
fn is_nan(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    Ok(Value::Bool(vm.to_number(&a)?.is_nan()))
}

fn trim_start(u: &[u16]) -> &[u16] {
    let mut i = 0;
    while i < u.len() && crate::unicode::is_str_whitespace(u[i] as u32) {
        i += 1;
    }
    &u[i..]
}

fn parse_float(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let t = trim_start(s.units());
    let bytes: Vec<u8> = t.iter().take_while(|&&c| c < 128).map(|&c| c as u8).collect();
    match numconv::parse_decimal_prefix(&bytes) {
        Some((v, _)) => Ok(Value::Number(v)),
        None => Ok(Value::Number(f64::NAN)),
    }
}

fn parse_int(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let ra = vm.arg(ctx, 1);
    let mut radix = vm.to_int32(&ra)?;
    let mut t = trim_start(s.units());
    let mut neg = false;
    if let Some(&c) = t.first() {
        if c == b'-' as u16 || c == b'+' as u16 {
            neg = c == b'-' as u16;
            t = &t[1..];
        }
    }
    let mut strip = true;
    if radix != 0 {
        if !(2..=36).contains(&radix) {
            return Ok(Value::Number(f64::NAN));
        }
        if radix != 16 {
            strip = false;
        }
    } else {
        radix = 10;
    }
    if strip && t.len() >= 2 && t[0] == b'0' as u16 && (t[1] == b'x' as u16 || t[1] == b'X' as u16) {
        t = &t[2..];
        radix = 16;
    }
    let digits: Vec<u8> = t.iter().take_while(|&&c| numconv::digit_val(c as u32).map(|d| d < radix as u32).unwrap_or(false)).map(|&c| c as u8).collect();
    if digits.is_empty() {
        return Ok(Value::Number(f64::NAN));
    }
    let v = numconv::parse_radix_int(&digits, radix as u32);
    Ok(Value::Number(if neg { -v } else { v }))
}

const URI_RESERVED: &str = ";/?:@&=+$,";
const URI_UNESCAPED_EXTRA: &str = "-_.!~*'()";

fn is_unreserved(c: u16) -> bool {
    c < 128 && ((c as u8).is_ascii_alphanumeric() || URI_UNESCAPED_EXTRA.as_bytes().contains(&(c as u8)))
}

fn encode(vm: &mut Vm, s: &JsStr, extra_unescaped: &str) -> JsResult<Value> {
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len());
    let mut k = 0;
    while k < u.len() {
        let c = u[k];
        if is_unreserved(c) || (c < 128 && extra_unescaped.as_bytes().contains(&(c as u8))) {
            out.push(c);
            k += 1;
            continue;
        }
        let (cp, n) = crate::string::code_point_at(u, k);
        if (0xD800..0xE000).contains(&cp) {
            return vm.throw_uri("URI malformed");
        }
        let mut buf = [0u8; 4];
        let ch = char::from_u32(cp).unwrap();
        for b in ch.encode_utf8(&mut buf).bytes() {
            out.push(b'%' as u16);
            out.extend(alloc::format!("{:02X}", b).encode_utf16());
        }
        k += n;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn decode(vm: &mut Vm, s: &JsStr, reserved: &str) -> JsResult<Value> {
    let u = s.units();
    let mut out: Vec<u16> = Vec::with_capacity(u.len());
    let mut k = 0;
    let hex = |c: u16| numconv::digit_val(c as u32).filter(|&d| d < 16);
    while k < u.len() {
        let c = u[k];
        if c != b'%' as u16 {
            out.push(c);
            k += 1;
            continue;
        }
        let start = k;
        let byte = |k: usize| -> Option<u8> {
            if k + 2 >= u.len() {
                return None;
            }
            if u[k] != b'%' as u16 {
                return None;
            }
            Some((hex(u[k + 1])? * 16 + hex(u[k + 2])?) as u8)
        };
        let b = match byte(k) {
            Some(b) => b,
            None => return vm.throw_uri("URI malformed"),
        };
        k += 3;
        if b < 0x80 {
            if reserved.as_bytes().contains(&b) {
                out.extend_from_slice(&u[start..k]);
            } else {
                out.push(b as u16);
            }
            continue;
        }
        let n = if b & 0xE0 == 0xC0 {
            2
        } else if b & 0xF0 == 0xE0 {
            3
        } else if b & 0xF8 == 0xF0 {
            4
        } else {
            return vm.throw_uri("URI malformed");
        };
        let mut bytes = alloc::vec![b];
        for _ in 1..n {
            match byte(k) {
                Some(x) if x & 0xC0 == 0x80 => {
                    bytes.push(x);
                    k += 3;
                }
                _ => return vm.throw_uri("URI malformed"),
            }
        }
        match core::str::from_utf8(&bytes) {
            Ok(st) => {
                let ch = st.chars().next().unwrap();
                crate::string::push_code_point(&mut out, ch as u32);
            }
            Err(_) => return vm.throw_uri("URI malformed"),
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn decode_uri(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    decode(vm, &s, ";/?:@&=+$,#")
}
fn decode_uri_component(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    decode(vm, &s, "")
}
fn encode_uri(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let extra = alloc::format!("{}#", URI_RESERVED);
    encode(vm, &s, &extra)
}
fn encode_uri_component(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    encode(vm, &s, "")
}

fn escape(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let mut out: Vec<u16> = Vec::new();
    for &c in s.units() {
        if c < 128 && ((c as u8).is_ascii_alphanumeric() || b"@*_+-./".contains(&(c as u8))) {
            out.push(c);
        } else if c < 256 {
            out.extend(alloc::format!("%{:02X}", c).encode_utf16());
        } else {
            out.extend(alloc::format!("%u{:04X}", c).encode_utf16());
        }
    }
    Ok(Value::String(JsStr::from_units(out)))
}

fn unescape(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a = vm.arg(ctx, 0);
    let s = vm.to_string(&a)?;
    let u = s.units();
    let hex = |c: u16| numconv::digit_val(c as u32).filter(|&d| d < 16);
    let mut out = Vec::with_capacity(u.len());
    let mut k = 0;
    while k < u.len() {
        let c = u[k];
        if c == b'%' as u16 {
            if k + 6 <= u.len() && u[k + 1] == b'u' as u16 {
                if let (Some(a), Some(b), Some(c2), Some(d)) = (hex(u[k + 2]), hex(u[k + 3]), hex(u[k + 4]), hex(u[k + 5])) {
                    out.push((a * 4096 + b * 256 + c2 * 16 + d) as u16);
                    k += 6;
                    continue;
                }
            }
            if k + 3 <= u.len() {
                if let (Some(a), Some(b)) = (hex(u[k + 1]), hex(u[k + 2])) {
                    out.push((a * 16 + b) as u16);
                    k += 3;
                    continue;
                }
            }
        }
        out.push(c);
        k += 1;
    }
    Ok(Value::String(JsStr::from_units(out)))
}

impl Vm {
    pub fn throw_uri<T>(&mut self, msg: &str) -> JsResult<T> {
        let p = self.intr().uri_error_proto;
        Err(self.make_error(p, msg))
    }
}
