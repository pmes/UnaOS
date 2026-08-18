use boa_engine::{
    Context, JsValue,
    object::ObjectInitializer,
    property::Attribute,
    native_function::NativeFunction,
    JsObject,
};

/// Installs the window/global surface.
///
/// `window` is the global object itself, exactly as in a browser: every
/// global — `setTimeout`, `document`, `fetch`, whatever a later prelude or
/// binding adds — is reachable as `window.x` for free, and `window.x = v`
/// creates a real global. Modelling it as a separate stub object (which is
/// what this used to do) makes `window.setTimeout` undefined while
/// `setTimeout` works, which is a difference no real page expects.
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

    let _ = context.register_global_property(
        boa_engine::string::JsString::from("location"),
        location,
        Attribute::all(),
    );

    let _ = context.register_global_property(
        boa_engine::string::JsString::from("history"),
        history,
        Attribute::all(),
    );

    // The prelude installs a fuller `navigator` (language, platform,
    // sendBeacon, ...); only fill in a minimal one if nothing is there, and
    // never clobber what is.
    let have_navigator = context
        .global_object()
        .get(boa_engine::string::JsString::from("navigator"), context)
        .map(|v| v.is_object())
        .unwrap_or(false);
    if !have_navigator {
        let navigator = ObjectInitializer::new(context)
            .property(
                boa_engine::string::JsString::from("userAgent"),
                boa_engine::string::JsString::from("UnaOS Aether/0.1.0"),
                Attribute::all(),
            )
            .build();
        let _ = context.register_global_property(
            boa_engine::string::JsString::from("navigator"),
            navigator,
            Attribute::all(),
        );
    }

    // window === globalThis === self === top === parent.
    let global: JsObject = context.global_object();
    for name in ["window", "self", "top", "parent"] {
        let _ = context.register_global_property(
            boa_engine::string::JsString::from(name),
            JsValue::from(global.clone()),
            Attribute::all(),
        );
    }
}
