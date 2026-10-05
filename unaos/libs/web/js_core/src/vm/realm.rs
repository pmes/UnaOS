//! Realms (§9.3): the intrinsics, the global object and the global environment record.

use super::value::{Obj, Value};
use crate::bytecode::Code;
use alloc::rc::Rc;
use alloc::vec::Vec;

macro_rules! intrinsics {
    ($($name:ident),* $(,)?) => {
        #[derive(Clone)]
        pub struct Intrinsics {
            $(pub $name: Obj,)*
            pub ta_protos: Vec<Obj>,
            pub ta_ctors: Vec<Obj>,
        }
        impl Intrinsics {
            pub fn placeholder() -> Intrinsics {
                Intrinsics { $($name: Obj(u32::MAX),)* ta_protos: Vec::new(), ta_ctors: Vec::new() }
            }
            pub fn trace(&self, out: &mut Vec<Obj>) {
                $(out.push(self.$name);)*
                out.extend_from_slice(&self.ta_protos);
                out.extend_from_slice(&self.ta_ctors);
            }
        }
    };
}

intrinsics!(
    object_proto, object_ctor, function_proto, function_ctor, array_proto, array_ctor, string_proto, string_ctor,
    number_proto, number_ctor, boolean_proto, boolean_ctor, symbol_proto, symbol_ctor, bigint_proto, bigint_ctor,
    error_proto, error_ctor, type_error_proto, type_error_ctor, range_error_proto, range_error_ctor,
    reference_error_proto, reference_error_ctor, syntax_error_proto, syntax_error_ctor, eval_error_proto,
    eval_error_ctor, uri_error_proto, uri_error_ctor, aggregate_error_proto, aggregate_error_ctor,
    iterator_proto, iterator_ctor, array_iterator_proto, map_iterator_proto, set_iterator_proto,
    string_iterator_proto, regexp_string_iterator_proto, generator_proto, async_generator_proto,
    generator_function_proto, generator_function_ctor, async_generator_function_proto,
    async_generator_function_ctor, async_function_proto, async_function_ctor, async_iterator_proto,
    async_from_sync_iterator_proto, iterator_helper_proto, wrap_for_valid_iterator_proto, promise_proto,
    promise_ctor, regexp_proto, regexp_ctor, date_proto, date_ctor, map_proto, map_ctor, set_proto, set_ctor,
    weakmap_proto, weakmap_ctor, weakset_proto, weakset_ctor, weakref_proto, weakref_ctor, finreg_proto,
    finreg_ctor, array_buffer_proto, array_buffer_ctor, shared_array_buffer_proto, shared_array_buffer_ctor,
    data_view_proto, data_view_ctor, typed_array_proto, typed_array_ctor, proxy_ctor, reflect, math, json,
    eval_fn, throw_type_error, array_proto_values, promise_then, object_proto_to_string, atomics,
    for_in_iterator_proto,
);

pub struct Realm {
    pub intrinsics: Intrinsics,
    pub global: Obj,
    pub global_env: Obj,
    pub global_this: Value,
    pub template_cache: Vec<(Rc<Code>, u32, Obj)>,
    /// Host-defined slot ($262 object etc.).
    pub host_defined: Value,
}

impl Realm {
    pub fn placeholder() -> Realm {
        Realm {
            intrinsics: Intrinsics::placeholder(),
            global: Obj(u32::MAX),
            global_env: Obj(u32::MAX),
            global_this: Value::Undefined,
            template_cache: Vec::new(),
            host_defined: Value::Undefined,
        }
    }

    pub fn trace(&self, out: &mut Vec<Obj>) {
        self.intrinsics.trace(out);
        out.push(self.global);
        out.push(self.global_env);
        if let Value::Object(o) = &self.global_this {
            out.push(*o);
        }
        if let Value::Object(o) = &self.host_defined {
            out.push(*o);
        }
        for (_, _, o) in &self.template_cache {
            out.push(*o);
        }
    }
}
