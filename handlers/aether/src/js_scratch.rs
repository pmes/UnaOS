use boa_engine::{
    Context, JsValue, JsResult,
    native_function::NativeFunction,
    object::ConstructorBuilder,
    string::JsString,
};

pub fn register(context: &mut Context) {
    let constructor = NativeFunction::from_fn_ptr(|_this, _args, _context| {
        Ok(JsValue::undefined())
    });
    let c = ConstructorBuilder::new(context, constructor)
        .name("EventTarget")
        .length(0)
        .build();
    let _ = JsValue::from(c.constructor()); // If it's a StandardConstructor, this works
}
