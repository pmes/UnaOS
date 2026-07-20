use boa_engine::{
    Context, JsValue,
    object::ObjectInitializer,
    property::Attribute,
    native_function::NativeFunction,
};

pub fn setup_window(context: &mut Context) {
    let location = ObjectInitializer::new(context)
        .property(
            boa_engine::string::JsString::from("href"),
            boa_engine::string::JsString::from("about:blank"),
            Attribute::all(),
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            boa_engine::string::JsString::from("reload"),
            0,
        )
        .build();

    let history = ObjectInitializer::new(context)
        .property(
            boa_engine::string::JsString::from("length"),
            1,
            Attribute::all(),
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            boa_engine::string::JsString::from("back"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            boa_engine::string::JsString::from("forward"),
            0,
        )
        .function(
            NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
            boa_engine::string::JsString::from("go"),
            1,
        )
        .build();

    let navigator = ObjectInitializer::new(context)
        .property(
            boa_engine::string::JsString::from("userAgent"),
            boa_engine::string::JsString::from("UnaOS Aether/0.1.0"),
            Attribute::all(),
        )
        .build();

    let window = ObjectInitializer::new(context)
        .property(
            boa_engine::string::JsString::from("location"),
            location.clone(),
            Attribute::all(),
        )
        .property(
            boa_engine::string::JsString::from("history"),
            history.clone(),
            Attribute::all(),
        )
        .property(
            boa_engine::string::JsString::from("navigator"),
            navigator.clone(),
            Attribute::all(),
        )
        .build();

    context.register_global_property(
        boa_engine::string::JsString::from("window"),
        window.clone(),
        Attribute::all(),
    );

    context.register_global_property(
        boa_engine::string::JsString::from("location"),
        location,
        Attribute::all(),
    );

    context.register_global_property(
        boa_engine::string::JsString::from("history"),
        history,
        Attribute::all(),
    );

    context.register_global_property(
        boa_engine::string::JsString::from("navigator"),
        navigator,
        Attribute::all(),
    );
}
