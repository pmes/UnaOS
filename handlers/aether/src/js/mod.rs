use boa_engine::{
    Context, JsValue, JsResult, JsError,
    native_function::NativeFunction,
    object::ObjectInitializer,
    property::Attribute,
    context::ContextBuilder,
};
use kuchiki::traits::*;
use kuchiki::NodeRef;
use std::cell::RefCell;
use std::collections::HashMap;

thread_local! {
    pub(crate) static DOM_STATE: RefCell<DomState> = RefCell::new(DomState {
        document: None,
        nodes: HashMap::new(),
        next_id: 1,
    });
}

pub(crate) struct DomState {
    pub(crate) document: Option<NodeRef>,
    pub(crate) nodes: HashMap<i32, NodeRef>,
    pub(crate) next_id: i32,
}

impl DomState {
    pub(crate) fn register_node(&mut self, node: NodeRef) -> i32 {
        let id = self.next_id;
        self.next_id += 1;
        self.nodes.insert(id, node);
        id
    }
    
    pub(crate) fn get_node(&self, id: i32) -> Option<NodeRef> {
        self.nodes.get(&id).cloned()
    }
}

pub struct Engine {
    pub context: Context,
}

impl Engine {
    pub fn new(document: NodeRef) -> Self {
        DOM_STATE.with(|state| {
            let mut state = state.borrow_mut();
            state.document = Some(document.clone());
            state.nodes.clear();
        });

        let mut context = ContextBuilder::new().build().unwrap();
        
        Self::setup_console(&mut context);
        Self::setup_document(&mut context, document);
        crate::api::window::setup_window(&mut context);
        // crate::api::cssom::setup_cssom(&mut context);
        crate::api::events::init(&mut context);
        
        Self { context }
    }

    fn setup_console(context: &mut Context) {
        crate::api::video::init(context);
        let console = ObjectInitializer::new(context)
            .function(
                NativeFunction::from_fn_ptr(|_, args, context| {
                    let mut output = String::new();
                    for arg in args {
                        output.push_str(&arg.to_string(context).unwrap().to_std_string_escaped());
                        output.push(' ');
                    }
                    println!("{}", output);
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("log"),
                0,
            )
            .build();
        context.register_global_property(boa_engine::string::JsString::from("console"), console, Attribute::all());

        crate::api::fetch::init(context);

        let set_timeout = NativeFunction::from_fn_ptr(|_, args, context| {
            if let Some(cb) = args.get(0).and_then(|v| v.as_callable()) {
                let _ = cb.call(&JsValue::undefined(), &[], context);
            }
            Ok(JsValue::new(1)) // return timer id
        });
        context.register_global_callable(boa_engine::string::JsString::from("setTimeout"), 2, set_timeout).unwrap();

        let clear_timeout = NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined()));
        context.register_global_callable(boa_engine::string::JsString::from("clearTimeout"), 1, clear_timeout).unwrap();
    }

    pub(crate) fn wrap_node(context: &mut Context, node: NodeRef) -> JsValue {
        let doc_id = DOM_STATE.with(|s| s.borrow_mut().register_node(node.clone()));
        
        let is_video = if let Some(el) = node.into_element_ref() {
            el.name.local.to_string() == "video"
        } else {
            false
        };

        let js_node = ObjectInitializer::new(context)
            .property(
                boa_engine::string::JsString::from("__node_id"),
                doc_id,
                Attribute::all(),
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let target_id = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let node = DOM_STATE.with(|s| s.borrow().get_node(id));
                    if let Some(n) = node {
                        if let Ok(mut matches) = n.select(&format!("#{}", target_id)) {
                            if let Some(first) = matches.next() {
                                return Ok(Self::wrap_node(ctx, first.as_node().clone()));
                            }
                        }
                    }
                    Ok(JsValue::null())
                }),
                boa_engine::string::JsString::from("getElementById"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let selector = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let node = DOM_STATE.with(|s| s.borrow().get_node(id));
                    if let Some(n) = node {
                        if let Ok(mut matches) = n.select(&selector) {
                            if let Some(first) = matches.next() {
                                return Ok(Self::wrap_node(ctx, first.as_node().clone()));
                            }
                        }
                    }
                    Ok(JsValue::null())
                }),
                boa_engine::string::JsString::from("querySelector"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, args, ctx| {
                    let tag = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let frag = kuchiki::parse_html().one(format!("<{}>", tag));
                    if let Ok(mut matches) = frag.select(&tag) {
                        if let Some(first) = matches.next() {
                            let node = first.as_node().clone();
                            node.detach();
                            return Ok(Self::wrap_node(ctx, node));
                        }
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("createElement"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let parent = DOM_STATE.with(|s| s.borrow().get_node(id));
                    
                    let child_id = args.get(0).and_then(|v| v.as_object()).and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok()).and_then(|v| v.as_number()).unwrap_or(0.0) as i32;
                    let child = DOM_STATE.with(|s| s.borrow().get_node(child_id));
                    
                    if let (Some(p), Some(c)) = (parent, child) {
                        p.append(c);
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("appendChild"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                boa_engine::string::JsString::from("innerHTML"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let node = DOM_STATE.with(|s| s.borrow().get_node(id));
                    if let Some(n) = node {
                        if args.is_empty() {
                            return Ok(JsValue::new(boa_engine::string::JsString::from(n.text_contents())));
                        } else {
                            let text = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                            for child in n.children() {
                                child.detach();
                            }
                            n.append(kuchiki::NodeRef::new_text(text));
                        }
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("textContent"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                boa_engine::string::JsString::from("setAttribute"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                boa_engine::string::JsString::from("addEventListener"),
                2,
            )
            .build();

        
        let style_obj = ObjectInitializer::new(context).build();
        let _ = js_node.set(boa_engine::string::JsString::from("style"), style_obj, false, context);

        js_node.into()
    }

    fn setup_document(context: &mut Context, doc: NodeRef) {
        let js_doc = Self::wrap_node(context, doc);
        context.register_global_property(boa_engine::string::JsString::from("document"), js_doc, Attribute::all());
    }
    
    pub fn execute(&mut self, script: &str) -> JsResult<JsValue> {
        use boa_engine::Source;
        self.context.eval(Source::from_bytes(script))
    }
}
