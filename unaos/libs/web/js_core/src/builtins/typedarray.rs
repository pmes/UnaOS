//! ArrayBuffer, SharedArrayBuffer, DataView, the TypedArray constructors and Atomics (§25.1–§25.4, §23.2).

use super::*;
use crate::vm::object::PropDesc;

pub fn init(_vm: &mut Vm) {}

/// CanonicalNumericIndexString for a property key: Some(index or -1 for non-integral numeric strings).
pub fn canonical_index(key: &PropertyKey) -> Option<f64> {
    match key {
        PropertyKey::Index(i) => Some(*i as f64),
        _ => None,
    }
}

pub fn ta_length(_vm: &Vm, _o: Obj) -> Option<usize> {
    None
}
pub fn is_fixed_length(_vm: &Vm, _o: Obj) -> bool {
    true
}
pub fn ta_valid_index(_vm: &Vm, _o: Obj, _i: f64) -> bool {
    false
}
pub fn ta_get_index(_vm: &Vm, _o: Obj, _i: f64) -> Option<Value> {
    None
}
pub fn ta_set_index(_vm: &mut Vm, _o: Obj, _i: f64, _v: &Value) -> JsResult<()> {
    Ok(())
}
pub fn ta_define_index(_vm: &mut Vm, _o: Obj, _i: f64, _d: PropDesc) -> JsResult<bool> {
    Ok(false)
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
