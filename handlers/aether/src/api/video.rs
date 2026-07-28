use boa_engine::{
    Context, JsValue,
    native_function::NativeFunction,
    object::ObjectInitializer,
    property::Attribute,
};

/// Media-element setup.
///
/// This used to register a global `HTMLVideoElement` whose call returned a
/// fresh playback object. Two things were wrong with that. `new
/// HTMLVideoElement()` throws "Illegal constructor" in every browser — the
/// name is an interface, not a factory — and registering it here overwrote
/// the real interface constructor the JS prelude installs, leaving the name
/// bound to a native function with no `prototype`. Any page doing
/// `el instanceof HTMLVideoElement` (player code does, constantly) got a
/// TypeError instead of a boolean, and the whole `HTML*Element` family lost
/// one member.
///
/// The interface constructor is the prelude's job, so there is nothing to
/// install here; the element factory below stays available for the media
/// lane to attach to real `<video>` nodes.
pub fn init(_context: &mut Context) {}

/// Creates a new JS object that represents a <video> element.
#[allow(dead_code)]
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
