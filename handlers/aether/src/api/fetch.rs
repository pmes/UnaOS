use boa_engine::{Context, native_function::NativeFunction};

pub fn init(context: &mut Context) {
    let _f = NativeFunction::from_fn_ptr(|_this, _args, ctx| {
        let o = boa_engine::object::ObjectInitializer::new(ctx).build();
        Ok(o.into())
    });
    
    // We stub out the Request/Response classes for now to unblock compilation
    context.register_global_callable("fetch".into(), 1, _f).unwrap();
}
