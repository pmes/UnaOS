use boa_engine::{
    Context, JsArgs, JsResult, JsValue, JsString,
    class::{Class, ClassBuilder},
    native_function::NativeFunction,
    property::Attribute,
    object::ObjectInitializer,
};
use boa_gc::{Finalize, Trace};

#[derive(Debug, Trace, Finalize)]
pub struct Element {
    tag_name: String,
}

impl boa_engine::object::JsData for Element {}

impl Class for Element {
    const NAME: &'static str = "Element";
    const LENGTH: usize = 1;

    fn data_constructor(_this: &JsValue, args: &[JsValue], _context: &mut Context) -> JsResult<Self> {
        let tag_name = args.get_or_undefined(0).to_string(_context)?.to_std_string_escaped();
        Ok(Self { tag_name })
    }

    fn init(class: &mut ClassBuilder<'_>) -> JsResult<()> {
        class.method(
            boa_engine::string::JsString::from("appendChild"),
            1,
            NativeFunction::from_fn_ptr(|_this, _args, _context| {
                Ok(JsValue::undefined())
            }),
        );
        
        class.property(
            boa_engine::string::JsString::from("innerHTML"),
            JsValue::undefined(),
            Attribute::all()
        );
        
        Ok(())
    }
}

pub fn init(context: &mut Context) {
    let _ = context.register_global_class::<Element>();
    
    // Stub document.createElement
    let create_element = NativeFunction::from_fn_ptr(|_this, args, context| {
        let tag_name = args.get_or_undefined(0);
        let element = Element { tag_name: tag_name.to_string(context)?.to_std_string_escaped() };
        let obj = ObjectInitializer::with_native_data(element, context).build();
        Ok(obj.into())
    });
    
    let js_doc = ObjectInitializer::new(context)
        .function(create_element, JsString::from("createElement"), 1)
        .build();
        
    let _ = context.register_global_property(JsString::from("document"), js_doc, Attribute::all());
}
