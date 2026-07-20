use boa_engine::{
    Context, JsValue,
    native_function::NativeFunction,
    object::ObjectInitializer,
    property::Attribute,
};

/// Initializes the global HTMLVideoElement constructor
pub fn init(context: &mut Context) {
    let video_constructor = NativeFunction::from_fn_ptr(|_, _, context| {
        Ok(create_video_element(context))
    });

    let _ = context.register_global_callable(
        boa_engine::string::JsString::from("HTMLVideoElement"),
        0,
        video_constructor,
    );
}

/// Creates a new JS object that represents a <video> element
pub fn create_video_element(context: &mut Context) -> JsValue {
    let obj = ObjectInitializer::new(context)
        .property(
            boa_engine::string::JsString::from("currentTime"),
            JsValue::new(0.0),
            Attribute::all(),
        )
        .property(
            boa_engine::string::JsString::from("paused"),
            JsValue::new(true),
            Attribute::all(),
        )
        .function(
            NativeFunction::from_fn_ptr(|this, _, ctx| {
                if let Some(obj) = this.as_object() {
                    let _ = obj.set(boa_engine::string::JsString::from("paused"), JsValue::new(false), false, ctx);
                }
                Ok(JsValue::undefined())
            }),
            boa_engine::string::JsString::from("play"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(|this, _, ctx| {
                if let Some(obj) = this.as_object() {
                    let _ = obj.set(boa_engine::string::JsString::from("paused"), JsValue::new(true), false, ctx);
                }
                Ok(JsValue::undefined())
            }),
            boa_engine::string::JsString::from("pause"),
            0,
        )
        .build();
    
    obj.into()
}
