use boa_engine::{
    Context, JsValue, JsResult,
    native_function::NativeFunction,
    object::ObjectInitializer,
    property::Attribute,
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
        mutated: false,
        handlers: Vec::new(),
    });
}

pub(crate) struct DomState {
    pub(crate) document: Option<NodeRef>,
    pub(crate) nodes: HashMap<i32, NodeRef>,
    pub(crate) next_id: i32,
    /// Set by mutating DOM bindings; the engine consumes it to relayout.
    pub(crate) mutated: bool,
    /// (node id, event name) registrations; the callbacks themselves live
    /// in the JS global `__handlers` array (GC objects must not outlive
    /// the boa context, so Rust never holds them).
    pub(crate) handlers: Vec<(i32, String)>,
}

/// True (and cleared) if the DOM was mutated by script since last asked.
pub fn take_mutated() -> bool {
    DOM_STATE.with(|s| std::mem::take(&mut s.borrow_mut().mutated))
}

/// Dispatches `event` to listeners on `node` and its ancestors (capture-less
/// bubble). Returns true if any handler ran. Registrations are matched via
/// DOM_STATE indices; the callbacks are fetched from the JS `__handlers`
/// array by position, so no GC object crosses into Rust storage.
pub fn dispatch_event(context: &mut Context, node: &NodeRef, event: &str) -> bool {
    // Find matching registration indices first — handlers may mutate the DOM.
    let mut indices = Vec::new();
    DOM_STATE.with(|s| {
        let s = s.borrow();
        let mut cur = Some(node.clone());
        while let Some(n) = cur {
            for (idx, (id, ev)) in s.handlers.iter().enumerate() {
                if ev == event {
                    if let Some(reg) = s.nodes.get(id) {
                        if reg == &n {
                            indices.push(idx);
                        }
                    }
                }
            }
            cur = n.parent();
        }
    });
    for idx in &indices {
        let _ = context.eval(boa_engine::Source::from_bytes(&format!(
            "if (globalThis.__handlers && globalThis.__handlers[{}]) globalThis.__handlers[{}]();",
            idx, idx
        )));
    }
    if !indices.is_empty() {
        DOM_STATE.with(|s| s.borrow_mut().mutated = true);
        true
    } else {
        false
    }
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
            state.handlers.clear();
            state.mutated = false;
        });

        let mut context = crate::event_loop::create_context();
        
        let _ = context.eval(boa_engine::Source::from_bytes("globalThis.__handlers = [];"));
        // Environment prelude: globals every real-world bundle expects.
        // requestAnimationFrame deliberately never fires its callback — an
        // immediate call turns rAF render loops into unbounded recursion.
        let _ = context.eval(boa_engine::Source::from_bytes(
            r#"
            globalThis.self = globalThis;
            globalThis.requestAnimationFrame = function (cb) { return 0; };
            globalThis.cancelAnimationFrame = function () {};
            globalThis.matchMedia = function (q) {
                // Honest answers for the two families sites branch on:
                // we are a light-scheme 800px-wide viewport. Everything
                // else is false.
                var m = false;
                var mw;
                if (/prefers-color-scheme:\s*light/.test(q)) { m = true; }
                else if ((mw = /min-width:\s*(\d+)/.exec(q))) { m = 800 >= +mw[1]; }
                else if ((mw = /max-width:\s*(\d+)/.exec(q))) { m = 800 <= +mw[1]; }
                return { matches: m, media: q,
                         addListener: function () {}, removeListener: function () {},
                         addEventListener: function () {}, removeEventListener: function () {} };
            };
            globalThis.navigator = {
                userAgent: 'UnaOS Aether/0.1.0',
                language: 'en-US', languages: ['en-US'],
                platform: 'UnaOS', cookieEnabled: false,
            };
            "#,
        ));
        Self::setup_console(&mut context);
        Self::setup_document(&mut context, document);
        crate::api::window::setup_window(&mut context);
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
        let _ = context.register_global_property(boa_engine::string::JsString::from("console"), console, Attribute::all());

        crate::api::fetch::init(context);

        let clear_timeout = NativeFunction::from_fn_ptr(|_, _, _| {
            crate::ledger::record_js("window.clearTimeout");
            Ok(JsValue::undefined())
        });
        context.register_global_callable(boa_engine::string::JsString::from("clearTimeout"), 1, clear_timeout).unwrap();
    }

    pub(crate) fn wrap_node(context: &mut Context, node: NodeRef) -> JsValue {
        let doc_id = DOM_STATE.with(|s| s.borrow_mut().register_node(node.clone()));
        
        let _is_video = if let Some(el) = node.into_element_ref() {
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
                        DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("appendChild"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let node = DOM_STATE.with(|s| s.borrow().get_node(id));
                    let Some(n) = node else { return Ok(JsValue::undefined()) };
                    if args.is_empty() {
                        // Getter: serialize children markup.
                        let html: String = n.children().map(|c| c.to_string()).collect();
                        return Ok(JsValue::new(boa_engine::string::JsString::from(html)));
                    }
                    let html = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    for child in n.children() {
                        child.detach();
                    }
                    // Parse as a document and graft the body's children.
                    let frag = kuchiki::parse_html().one(html);
                    if let Ok(mut bodies) = frag.select("body") {
                        if let Some(body) = bodies.next() {
                            let children: Vec<_> = body.as_node().children().collect();
                            for child in children {
                                child.detach();
                                n.append(child);
                            }
                        }
                    }
                    DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                    Ok(JsValue::undefined())
                }),
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
                            DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                        }
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("textContent"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let name = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let value = args.get(1).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        if let Some(el) = n.as_element() {
                            el.attributes.borrow_mut().insert(name.as_str(), value);
                            DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                        }
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("setAttribute"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let name = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        if let Some(el) = n.as_element() {
                            if let Some(v) = el.attributes.borrow().get(name.as_str()) {
                                return Ok(JsValue::new(boa_engine::string::JsString::from(v)));
                            }
                        }
                    }
                    Ok(JsValue::null())
                }),
                boa_engine::string::JsString::from("getAttribute"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let event = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    if let Some(cb) = args.get(1).and_then(|v| v.as_object()) {
                        // The callback lives in the JS __handlers array; Rust
                        // records only (node id, event) at the same index.
                        let global = ctx.global_object();
                        if let Ok(arr_val) = global.get(boa_engine::string::JsString::from("__handlers"), ctx) {
                            if let Some(arr) = arr_val.as_object() {
                                if let Ok(len_v) = arr.get(boa_engine::string::JsString::from("length"), ctx) {
                                    let len = len_v.as_number().unwrap_or(0.0) as u32;
                                    let _ = arr.set(len, JsValue::from(cb.clone()), false, ctx);
                                    DOM_STATE.with(|s| s.borrow_mut().handlers.push((id, event)));
                                }
                            }
                        }
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("addEventListener"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let selector = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let arr = boa_engine::object::builtins::JsArray::new(ctx);
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        if let Ok(matches) = n.select(&selector) {
                            for m in matches.take(256) {
                                let wrapped = Self::wrap_node(ctx, m.as_node().clone());
                                let _ = arr.push(wrapped, ctx);
                            }
                        }
                    }
                    Ok(arr.into())
                }),
                boa_engine::string::JsString::from("querySelectorAll"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let name = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        if let Some(el) = n.as_element() {
                            el.attributes.borrow_mut().remove(name.as_str());
                            DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                        }
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("removeAttribute"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let child_id = args.get(0).and_then(|v| v.as_object()).and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok()).and_then(|v| v.as_number()).unwrap_or(-1.0) as i32;
                    let (parent, child) = DOM_STATE.with(|s| (s.borrow().get_node(id), s.borrow().get_node(child_id)));
                    if let (Some(p), Some(c)) = (parent, child) {
                        if c.parent().map_or(false, |cp| cp == p) {
                            c.detach();
                            DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                        }
                    }
                    Ok(args.get(0).cloned().unwrap_or_default())
                }),
                boa_engine::string::JsString::from("removeChild"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    // className(): getter; className(v): setter.
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) else { return Ok(JsValue::undefined()) };
                    let Some(el) = n.as_element() else { return Ok(JsValue::undefined()) };
                    if let Some(v) = args.get(0) {
                        let v = v.to_string(ctx).unwrap_or_default().to_std_string_escaped();
                        el.attributes.borrow_mut().insert("class", v);
                        DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                        Ok(JsValue::undefined())
                    } else {
                        let v = el.attributes.borrow().get("class").unwrap_or("").to_string();
                        Ok(JsValue::new(boa_engine::string::JsString::from(v)))
                    }
                }),
                boa_engine::string::JsString::from("className"),
                1,
            )
            .build();

        // classList: add/remove/toggle/contains over the class attribute.
        let class_list = ObjectInitializer::new(context)
            .property(boa_engine::string::JsString::from("__node_id"), doc_id, Attribute::all())
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| { Self::class_op(this, args, ctx, "add") }),
                boa_engine::string::JsString::from("add"), 1)
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| { Self::class_op(this, args, ctx, "remove") }),
                boa_engine::string::JsString::from("remove"), 1)
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| { Self::class_op(this, args, ctx, "toggle") }),
                boa_engine::string::JsString::from("toggle"), 1)
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| { Self::class_op(this, args, ctx, "contains") }),
                boa_engine::string::JsString::from("contains"), 1)
            .build();
        let _ = js_node.set(boa_engine::string::JsString::from("classList"), class_list, false, context);

        
        let style_obj = ObjectInitializer::new(context).build();
        let _ = js_node.set(boa_engine::string::JsString::from("style"), style_obj, false, context);

        js_node.into()
    }

    /// Shared classList operation over the class attribute.
    fn class_op(this: &JsValue, args: &[JsValue], ctx: &mut Context, op: &str) -> JsResult<JsValue> {
        let id = this
            .as_object()
            .and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok())
            .and_then(|v| v.as_number())
            .unwrap_or(0.0) as i32;
        let name = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
        let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) else { return Ok(JsValue::undefined()) };
        let Some(el) = n.as_element() else { return Ok(JsValue::undefined()) };
        let current = el.attributes.borrow().get("class").unwrap_or("").to_string();
        let mut classes: Vec<&str> = current.split_whitespace().collect();
        let has = classes.contains(&name.as_str());
        match op {
            "contains" => return Ok(JsValue::from(has)),
            "add" if !has => classes.push(&name),
            "remove" => classes.retain(|c| *c != name),
            "toggle" => {
                if has { classes.retain(|c| *c != name); } else { classes.push(&name); }
            }
            _ => {}
        }
        let joined = classes.join(" ");
        el.attributes.borrow_mut().insert("class", joined);
        DOM_STATE.with(|s| s.borrow_mut().mutated = true);
        if op == "toggle" {
            return Ok(JsValue::from(!has));
        }
        Ok(JsValue::undefined())
    }

    fn setup_document(context: &mut Context, doc: NodeRef) {
        // documentElement / body / head wrapped as real node properties —
        // `document.documentElement.classList.add('js')` is the canonical
        // app-shell boot line.
        let mut extras: Vec<(&str, NodeRef)> = Vec::new();
        for (prop, sel) in [("documentElement", "html"), ("body", "body"), ("head", "head")] {
            if let Ok(mut m) = doc.select(sel) {
                if let Some(el) = m.next() {
                    extras.push((prop, el.as_node().clone()));
                }
            }
        }
        let js_doc = Self::wrap_node(context, doc);
        if let Some(obj) = js_doc.as_object() {
            for (prop, node) in extras {
                let wrapped = Self::wrap_node(context, node);
                let _ = obj.set(boa_engine::string::JsString::from(prop), wrapped, false, context);
            }
        }
        let _ = context.register_global_property(boa_engine::string::JsString::from("document"), js_doc, Attribute::all());
    }
    
    pub fn execute(&mut self, script: &str) -> JsResult<JsValue> {
        use boa_engine::Source;
        self.context.eval(Source::from_bytes(script))
    }
}
