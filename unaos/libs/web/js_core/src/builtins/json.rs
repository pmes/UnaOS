//! JSON (§25.5): parse with reviver, stringify with replacer / indentation.

use super::*;

pub fn init(vm: &mut Vm) {
    let op = vm.intr().object_proto;
    let j = vm.new_object(Some(op));
    method(vm, j, "parse", 2, parse);
    method(vm, j, "stringify", 3, stringify);
    to_str_tag(vm, j, "JSON");
    let r = vm.cur_realm as usize;
    vm.realms[r].intrinsics.json = j;
    global(vm, "JSON", Value::Object(j));
}

struct P<'a> {
    s: &'a [u16],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], 0x09 | 0x0A | 0x0D | 0x20) {
            self.i += 1;
        }
    }
    fn peek(&self) -> Option<u16> {
        self.s.get(self.i).copied()
    }
}

/// Parse JSON text into a value (also used by JSON modules).
pub fn parse_json_text(vm: &mut Vm, text: &JsStr) -> JsResult<Value> {
    let mut p = P { s: text.units(), i: 0 };
    p.ws();
    let v = parse_value(vm, &mut p, 0)?;
    p.ws();
    if p.i != p.s.len() {
        return vm.throw_syntax("Unexpected token in JSON");
    }
    Ok(v)
}

fn parse_value(vm: &mut Vm, p: &mut P, depth: usize) -> JsResult<Value> {
    if depth > 4000 {
        return vm.throw_range("JSON nesting too deep");
    }
    p.ws();
    match p.peek() {
        Some(0x7B) => {
            p.i += 1;
            let o = vm.new_plain_object();
            vm.root(&Value::Object(o));
            p.ws();
            if p.peek() == Some(0x7D) {
                p.i += 1;
                return Ok(Value::Object(o));
            }
            loop {
                p.ws();
                if p.peek() != Some(0x22) {
                    return vm.throw_syntax("Expected property name in JSON");
                }
                let k = parse_string(vm, p)?;
                p.ws();
                if p.peek() != Some(0x3A) {
                    return vm.throw_syntax("Expected ':' in JSON");
                }
                p.i += 1;
                let v = parse_value(vm, p, depth + 1)?;
                vm.create_data_property(o, PropertyKey::from_js(k), v)?;
                p.ws();
                match p.peek() {
                    Some(0x2C) => {
                        p.i += 1;
                    }
                    Some(0x7D) => {
                        p.i += 1;
                        return Ok(Value::Object(o));
                    }
                    _ => return vm.throw_syntax("Expected ',' or '}' in JSON"),
                }
            }
        }
        Some(0x5B) => {
            p.i += 1;
            let a = vm.new_array(Vec::new());
            vm.root(&Value::Object(a));
            p.ws();
            if p.peek() == Some(0x5D) {
                p.i += 1;
                return Ok(Value::Object(a));
            }
            loop {
                let v = parse_value(vm, p, depth + 1)?;
                if let Kind::Array(ad) = &mut vm.heap.get_mut(a).kind {
                    ad.elems.push(v);
                }
                p.ws();
                match p.peek() {
                    Some(0x2C) => {
                        p.i += 1;
                    }
                    Some(0x5D) => {
                        p.i += 1;
                        return Ok(Value::Object(a));
                    }
                    _ => return vm.throw_syntax("Expected ',' or ']' in JSON"),
                }
            }
        }
        Some(0x22) => Ok(Value::String(parse_string(vm, p)?)),
        Some(c) if c == b'-' as u16 || (0x30..=0x39).contains(&c) => {
            let start = p.i;
            if p.peek() == Some(b'-' as u16) {
                p.i += 1;
            }
            match p.peek() {
                Some(0x30) => p.i += 1,
                Some(c) if (0x31..=0x39).contains(&c) => {
                    while matches!(p.peek(), Some(c) if (0x30..=0x39).contains(&c)) {
                        p.i += 1;
                    }
                }
                _ => return vm.throw_syntax("Invalid number in JSON"),
            }
            if p.peek() == Some(b'.' as u16) {
                p.i += 1;
                if !matches!(p.peek(), Some(c) if (0x30..=0x39).contains(&c)) {
                    return vm.throw_syntax("Invalid number in JSON");
                }
                while matches!(p.peek(), Some(c) if (0x30..=0x39).contains(&c)) {
                    p.i += 1;
                }
            }
            if matches!(p.peek(), Some(0x65) | Some(0x45)) {
                p.i += 1;
                if matches!(p.peek(), Some(0x2B) | Some(0x2D)) {
                    p.i += 1;
                }
                if !matches!(p.peek(), Some(c) if (0x30..=0x39).contains(&c)) {
                    return vm.throw_syntax("Invalid number in JSON");
                }
                while matches!(p.peek(), Some(c) if (0x30..=0x39).contains(&c)) {
                    p.i += 1;
                }
            }
            let bytes: Vec<u8> = p.s[start..p.i].iter().map(|&c| c as u8).collect();
            let (v, _) = crate::numconv::parse_decimal_prefix(&bytes).unwrap_or((f64::NAN, 0));
            Ok(Value::Number(v))
        }
        _ => {
            for (lit, v) in [("true", Value::Bool(true)), ("false", Value::Bool(false)), ("null", Value::Null)] {
                let u: Vec<u16> = lit.encode_utf16().collect();
                if p.s[p.i..].starts_with(&u) {
                    p.i += u.len();
                    return Ok(v);
                }
            }
            vm.throw_syntax("Unexpected token in JSON")
        }
    }
}

fn parse_string(vm: &mut Vm, p: &mut P) -> JsResult<JsStr> {
    p.i += 1;
    let mut out = Vec::new();
    loop {
        let c = match p.peek() {
            Some(c) => c,
            None => return vm.throw_syntax("Unterminated string in JSON"),
        };
        p.i += 1;
        match c {
            0x22 => return Ok(JsStr::from_units(out)),
            0x5C => {
                let e = match p.peek() {
                    Some(e) => e,
                    None => return vm.throw_syntax("Bad escape in JSON"),
                };
                p.i += 1;
                match e {
                    0x22 | 0x5C | 0x2F => out.push(e),
                    0x62 => out.push(8),
                    0x66 => out.push(12),
                    0x6E => out.push(10),
                    0x72 => out.push(13),
                    0x74 => out.push(9),
                    0x75 => {
                        let mut v = 0u32;
                        for _ in 0..4 {
                            let d = p.peek().and_then(|c| crate::numconv::digit_val(c as u32)).filter(|&d| d < 16);
                            match d {
                                Some(d) => v = v * 16 + d,
                                None => return vm.throw_syntax("Bad unicode escape in JSON"),
                            }
                            p.i += 1;
                        }
                        out.push(v as u16);
                    }
                    _ => return vm.throw_syntax("Bad escape in JSON"),
                }
            }
            c if c < 0x20 => return vm.throw_syntax("Bad control character in string literal in JSON"),
            c => out.push(c),
        }
    }
}

fn parse(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = vm.arg(ctx, 0);
    let text = vm.to_string(&t)?;
    let v = parse_json_text(vm, &text)?;
    let reviver = vm.arg(ctx, 1);
    if vm.is_callable(&reviver) {
        let root = vm.new_plain_object();
        vm.create_data_property_or_throw(root, PropertyKey::from_str(""), v)?;
        return internalize(vm, root, PropertyKey::from_str(""), &reviver);
    }
    Ok(v)
}

fn internalize(vm: &mut Vm, holder: Obj, name: PropertyKey, reviver: &Value) -> JsResult<Value> {
    let val = vm.get(holder, &name)?;
    if let Value::Object(o) = &val {
        let o = *o;
        if vm.is_array(&val)? {
            let len = vm.length_of(o)?;
            let mut i = 0.0;
            while i < len {
                let k = crate::builtins::array::key_of(i);
                let ne = internalize(vm, o, k.clone(), reviver)?;
                if ne.is_undefined() {
                    vm.delete(o, &k)?;
                } else {
                    vm.create_data_property(o, k, ne)?;
                }
                i += 1.0;
            }
        } else {
            let keys = vm.enumerable_own_keys(o)?;
            for k in keys {
                let ne = internalize(vm, o, k.clone(), reviver)?;
                if ne.is_undefined() {
                    vm.delete(o, &k)?;
                } else {
                    vm.create_data_property(o, k, ne)?;
                }
            }
        }
    }
    vm.call(reviver, &Value::Object(holder), &[name.to_value(), val])
}

struct Ser {
    replacer: Option<Value>,
    keys: Option<Vec<PropertyKey>>,
    gap: Vec<u16>,
    indent: Vec<u16>,
    stack: Vec<Obj>,
}

fn stringify(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let value = vm.arg(ctx, 0);
    let replacer = vm.arg(ctx, 1);
    let space = vm.arg(ctx, 2);
    let mut st = Ser { replacer: None, keys: None, gap: Vec::new(), indent: Vec::new(), stack: Vec::new() };
    if let Value::Object(ro) = &replacer {
        if vm.is_callable(&replacer) {
            st.replacer = Some(replacer.clone());
        } else if vm.is_array(&replacer)? {
            let len = vm.length_of(*ro)?;
            let mut list: Vec<PropertyKey> = Vec::new();
            let mut i = 0.0;
            while i < len {
                let v = vm.get(*ro, &crate::builtins::array::key_of(i))?;
                let item = match &v {
                    Value::String(s) => Some(s.clone()),
                    Value::Number(n) => Some(crate::vm::ops::number_to_jsstr(*n)),
                    Value::Object(o) => match vm.heap.get(*o).kind {
                        Kind::String(_) | Kind::Number(_) => Some(vm.to_string(&v)?),
                        _ => None,
                    },
                    _ => None,
                };
                if let Some(s) = item {
                    let k = PropertyKey::from_js(s);
                    if !list.contains(&k) {
                        list.push(k);
                    }
                }
                i += 1.0;
            }
            st.keys = Some(list);
        }
    }
    let mut space = space;
    if let Value::Object(so) = &space {
        match vm.heap.get(*so).kind {
            Kind::Number(_) => space = Value::Number(vm.to_number(&space)?),
            Kind::String(_) => space = Value::String(vm.to_string(&space)?),
            _ => {}
        }
    }
    match &space {
        Value::Number(n) => {
            let k = crate::vm::ops::integer_or_infinity(*n).clamp(0.0, 10.0) as usize;
            st.gap = alloc::vec![0x20; k];
        }
        Value::String(s) => {
            st.gap = s.units()[..s.len().min(10)].to_vec();
        }
        _ => {}
    }
    let wrapper = vm.new_plain_object();
    vm.create_data_property_or_throw(wrapper, PropertyKey::from_str(""), value)?;
    vm.root(&Value::Object(wrapper));
    let mut out = Vec::new();
    let ok = ser_property(vm, &mut st, wrapper, PropertyKey::from_str(""), &mut out)?;
    if !ok {
        return Ok(Value::Undefined);
    }
    Ok(Value::String(JsStr::from_units(out)))
}

/// SerializeJSONProperty: returns false for undefined (nothing written).
fn ser_property(vm: &mut Vm, st: &mut Ser, holder: Obj, key: PropertyKey, out: &mut Vec<u16>) -> JsResult<bool> {
    let mut value = vm.get(holder, &key)?;
    if value.is_object() || matches!(value, Value::BigInt(_)) {
        let tj = vm.get_v(&value, &PropertyKey::from_str("toJSON"))?;
        if vm.is_callable(&tj) {
            value = vm.call(&tj, &value, &[key.to_value()])?;
        }
    }
    if let Some(r) = st.replacer.clone() {
        value = vm.call(&r, &Value::Object(holder), &[key.to_value(), value])?;
    }
    if let Value::Object(o) = &value {
        let o = *o;
        match &vm.heap.get(o).kind {
            Kind::Number(_) => value = Value::Number(vm.to_number(&value)?),
            Kind::String(_) => value = Value::String(vm.to_string(&value)?),
            Kind::Boolean(b) => value = Value::Bool(*b),
            Kind::BigInt(b) => value = Value::BigInt(b.clone()),
            _ => {}
        }
    }
    match &value {
        Value::Null => out.extend("null".encode_utf16()),
        Value::Bool(b) => out.extend(if *b { "true" } else { "false" }.encode_utf16()),
        Value::String(s) => quote(s.units(), out),
        Value::Number(n) => {
            if n.is_finite() {
                out.extend_from_slice(crate::vm::ops::number_to_jsstr(*n).units());
            } else {
                out.extend("null".encode_utf16());
            }
        }
        Value::BigInt(_) => return vm.throw_type("Do not know how to serialize a BigInt"),
        Value::Object(o) if !vm.is_callable(&value) => {
            let o = *o;
            if vm.is_array(&value)? {
                ser_array(vm, st, o, out)?;
            } else {
                ser_object(vm, st, o, out)?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

pub fn quote(s: &[u16], out: &mut Vec<u16>) {
    out.push(0x22);
    let mut i = 0;
    while i < s.len() {
        let c = s[i];
        match c {
            0x08 => out.extend("\\b".encode_utf16()),
            0x09 => out.extend("\\t".encode_utf16()),
            0x0A => out.extend("\\n".encode_utf16()),
            0x0C => out.extend("\\f".encode_utf16()),
            0x0D => out.extend("\\r".encode_utf16()),
            0x22 => out.extend("\\\"".encode_utf16()),
            0x5C => out.extend("\\\\".encode_utf16()),
            c if c < 0x20 => out.extend(alloc::format!("\\u{:04x}", c).encode_utf16()),
            c if (0xD800..0xDC00).contains(&c) => {
                if i + 1 < s.len() && (0xDC00..0xE000).contains(&s[i + 1]) {
                    out.push(c);
                    out.push(s[i + 1]);
                    i += 1;
                } else {
                    out.extend(alloc::format!("\\u{:04x}", c).encode_utf16());
                }
            }
            c if (0xDC00..0xE000).contains(&c) => out.extend(alloc::format!("\\u{:04x}", c).encode_utf16()),
            c => out.push(c),
        }
        i += 1;
    }
    out.push(0x22);
}

fn ser_object(vm: &mut Vm, st: &mut Ser, o: Obj, out: &mut Vec<u16>) -> JsResult<()> {
    if st.stack.contains(&o) {
        return vm.throw_type("Converting circular structure to JSON");
    }
    if st.stack.len() > 4000 {
        return vm.throw_range("Maximum call stack size exceeded");
    }
    st.stack.push(o);
    let stepback = st.indent.clone();
    st.indent.extend_from_slice(&st.gap.clone());
    let keys = match &st.keys {
        Some(k) => k.clone(),
        None => vm.enumerable_own_keys(o)?,
    };
    let mut parts: Vec<Vec<u16>> = Vec::new();
    for k in keys {
        let mut item = Vec::new();
        quote(k.to_js_string().units(), &mut item);
        item.push(b':' as u16);
        if !st.gap.is_empty() {
            item.push(b' ' as u16);
        }
        if ser_property(vm, st, o, k, &mut item)? {
            parts.push(item);
        }
    }
    join_parts(st, &parts, &stepback, 0x7B, 0x7D, out);
    st.stack.pop();
    st.indent = stepback;
    Ok(())
}

fn ser_array(vm: &mut Vm, st: &mut Ser, o: Obj, out: &mut Vec<u16>) -> JsResult<()> {
    if st.stack.contains(&o) {
        return vm.throw_type("Converting circular structure to JSON");
    }
    if st.stack.len() > 4000 {
        return vm.throw_range("Maximum call stack size exceeded");
    }
    st.stack.push(o);
    let stepback = st.indent.clone();
    st.indent.extend_from_slice(&st.gap.clone());
    let len = vm.length_of(o)?;
    let mut parts: Vec<Vec<u16>> = Vec::new();
    let mut i = 0.0;
    while i < len {
        let mut item = Vec::new();
        if !ser_property(vm, st, o, crate::builtins::array::key_of(i), &mut item)? {
            item.extend("null".encode_utf16());
        }
        parts.push(item);
        i += 1.0;
    }
    join_parts(st, &parts, &stepback, 0x5B, 0x5D, out);
    st.stack.pop();
    st.indent = stepback;
    Ok(())
}

fn join_parts(st: &Ser, parts: &[Vec<u16>], stepback: &[u16], open: u16, close: u16, out: &mut Vec<u16>) {
    out.push(open);
    if parts.is_empty() {
        out.push(close);
        return;
    }
    if st.gap.is_empty() {
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                out.push(b',' as u16);
            }
            out.extend_from_slice(p);
        }
    } else {
        for (i, p) in parts.iter().enumerate() {
            if i > 0 {
                out.push(b',' as u16);
            }
            out.push(0x0A);
            out.extend_from_slice(&st.indent);
            out.extend_from_slice(p);
        }
        out.push(0x0A);
        out.extend_from_slice(stepback);
    }
    out.push(close);
}
