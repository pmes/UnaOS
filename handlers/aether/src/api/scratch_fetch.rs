use boa_engine::{Context, JsResult, JsValue, native_function::NativeFunction};

fn test_async() {
    let f = NativeFunction::from_async_fn(|_this, _args, ctx| async move {
        // Can we use ctx here?
        let o = boa_engine::object::ObjectInitializer::new(ctx).build();
        Ok(o.into())
    });
}
