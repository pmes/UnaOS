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

        crate::api::fetch::reset_budget();
        let mut context = crate::event_loop::create_context();
        
        let _ = context.eval(boa_engine::Source::from_bytes("globalThis.__handlers = [];"));
        // Environment prelude: globals every real-world bundle expects.
        // requestAnimationFrame deliberately never fires its callback — an
        // immediate call turns rAF render loops into unbounded recursion.
        let _ = context.eval(boa_engine::Source::from_bytes(
            r#"
            globalThis.self = globalThis;
            // rAF: callbacks queue and fire in bounded passes at load
            // (see __drainRaf) — an unbounded immediate fire would turn
            // render loops into recursion; never firing starved
            // framework paints.
            globalThis.__raf = [];
            globalThis.__rafClock = 0;
            globalThis.requestAnimationFrame = function (cb) {
                __raf.push(cb);
                return __raf.length;
            };
            globalThis.cancelAnimationFrame = function (id) {
                if (id > 0 && id <= __raf.length) { __raf[id - 1] = null; }
            };
            // Bounded drain: each pass runs the current batch once; new
            // registrations wait for the next pass; after 8 passes what
            // keeps re-registering is an animation loop and is dropped.
            globalThis.__drainRaf = function () {
                var passes = 0;
                while (__raf.length && passes < 8) {
                    var batch = __raf;
                    __raf = [];
                    __rafClock += 16;
                    for (var i = 0; i < batch.length; i++) {
                        if (batch[i]) { try { batch[i](__rafClock); } catch (e) {} }
                    }
                    passes++;
                }
                __raf = [];
            };
            globalThis.performance = {
                now: function () { return __rafClock; },
                mark: function () {}, measure: function () {},
                getEntriesByName: function () { return []; },
                getEntriesByType: function () { return []; },
                timing: {}, timeOrigin: 0,
            };
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
            globalThis.Image = function (w, h) {
                this.width = w || 0; this.height = h || 0;
                this.src = ''; this.complete = false;
                this.addEventListener = function () {};
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

    /// Writes one declaration into a node's inline style attribute,
    /// replacing any existing declaration for the property.
    fn set_style_prop(node: &NodeRef, prop: &str, value: &str) {
        let Some(el) = node.as_element() else { return };
        let current = el.attributes.borrow().get("style").unwrap_or("").to_string();
        let mut decls: Vec<String> = current
            .split(';')
            .filter_map(|d| {
                let d = d.trim();
                if d.is_empty() {
                    return None;
                }
                match d.split_once(':') {
                    Some((p, _)) if p.trim().eq_ignore_ascii_case(prop) => None,
                    _ => Some(d.to_string()),
                }
            })
            .collect();
        if !value.trim().is_empty() {
            decls.push(format!("{}: {}", prop, value.trim()));
        }
        el.attributes.borrow_mut().insert("style", decls.join("; "));
        DOM_STATE.with(|s| s.borrow_mut().mutated = true);
    }

    /// Reads one declaration back from the inline style attribute.
    fn get_style_prop(node: &NodeRef, prop: &str) -> String {
        let Some(el) = node.as_element() else { return String::new() };
        let current = el.attributes.borrow().get("style").unwrap_or("").to_string();
        for d in current.split(';') {
            if let Some((p, v)) = d.split_once(':') {
                if p.trim().eq_ignore_ascii_case(prop) {
                    return v.trim().to_string();
                }
            }
        }
        String::new()
    }

    /// Resolves the DOM node behind a wrapped JS object (via __node_id).
    fn this_node(this: &JsValue, ctx: &mut Context) -> Option<NodeRef> {
        let id = this
            .as_object()?
            .get(boa_engine::string::JsString::from("__node_id"), ctx)
            .ok()?
            .as_number()? as i32;
        DOM_STATE.with(|s| s.borrow().get_node(id))
    }

    /// Builds a native JsFunction for accessor slots.
    fn native(
        context: &mut Context,
        f: fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue>,
    ) -> boa_engine::object::builtins::JsFunction {
        boa_engine::object::FunctionObjectBuilder::new(
            context.realm(),
            NativeFunction::from_fn_ptr(f),
        )
        .build()
    }

    pub(crate) fn wrap_node(context: &mut Context, node: NodeRef) -> JsValue {
        let doc_id = DOM_STATE.with(|s| s.borrow_mut().register_node(node.clone()));
        
        let _is_video = if let Some(el) = node.into_element_ref() {
            el.name.local.to_string() == "video"
        } else {
            false
        };

        // Accessor pairs must exist before ObjectInitializer borrows context.
        let get_inner_html = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let html: String = n.children().map(|c| c.to_string()).collect();
            Ok(JsValue::new(boa_engine::string::JsString::from(html)))
        });
        let set_inner_html = Self::native(context, |this, args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let html = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
            for child in n.children() {
                child.detach();
            }
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
        });
        let get_text = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            Ok(JsValue::new(boa_engine::string::JsString::from(n.text_contents())))
        });
        let set_text = Self::native(context, |this, args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let text = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
            for child in n.children() {
                child.detach();
            }
            n.append(kuchiki::NodeRef::new_text(text));
            DOM_STATE.with(|s| s.borrow_mut().mutated = true);
            Ok(JsValue::undefined())
        });
        let get_class = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let v = n.as_element().and_then(|el| el.attributes.borrow().get("class").map(|s| s.to_string())).unwrap_or_default();
            Ok(JsValue::new(boa_engine::string::JsString::from(v)))
        });
        let set_class = Self::native(context, |this, args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let v = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
            if let Some(el) = n.as_element() {
                el.attributes.borrow_mut().insert("class", v);
                DOM_STATE.with(|s| s.borrow_mut().mutated = true);
            }
            Ok(JsValue::undefined())
        });
        let get_id_attr = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let v = n.as_element().and_then(|el| el.attributes.borrow().get("id").map(|s| s.to_string())).unwrap_or_default();
            Ok(JsValue::new(boa_engine::string::JsString::from(v)))
        });
        let set_id_attr = Self::native(context, |this, args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let v = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
            if let Some(el) = n.as_element() {
                el.attributes.borrow_mut().insert("id", v);
                DOM_STATE.with(|s| s.borrow_mut().mutated = true);
            }
            Ok(JsValue::undefined())
        });
        let get_value = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let v = n.as_element().and_then(|el| el.attributes.borrow().get("value").map(|s| s.to_string())).unwrap_or_default();
            Ok(JsValue::new(boa_engine::string::JsString::from(v)))
        });
        let set_value = Self::native(context, |this, args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let v = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
            if let Some(el) = n.as_element() {
                el.attributes.borrow_mut().insert("value", v);
                DOM_STATE.with(|s| s.borrow_mut().mutated = true);
            }
            Ok(JsValue::undefined())
        });

        let get_parent = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::null()) };
            match n.parent() {
                Some(parent) => Ok(Self::wrap_node(ctx, parent)),
                None => Ok(JsValue::null()),
            }
        });
        let get_children = Self::native(context, |this, _args, ctx| {
            let arr = boa_engine::object::builtins::JsArray::new(ctx);
            if let Some(n) = Self::this_node(this, ctx) {
                for child in n.children().filter(|c| c.as_element().is_some()).take(256) {
                    let wrapped = Self::wrap_node(ctx, child);
                    let _ = arr.push(wrapped, ctx);
                }
            }
            Ok(arr.into())
        });
        let get_first_child = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::null()) };
            match n.children().find(|c| c.as_element().is_some()) {
                Some(c) => Ok(Self::wrap_node(ctx, c)),
                None => Ok(JsValue::null()),
            }
        });
        let get_next_sibling = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::null()) };
            let mut cur = n.next_sibling();
            while let Some(sib) = cur {
                if sib.as_element().is_some() {
                    return Ok(Self::wrap_node(ctx, sib));
                }
                cur = sib.next_sibling();
            }
            Ok(JsValue::null())
        });
        let get_tag = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            let tag = n
                .as_element()
                .map(|el| el.name.local.as_ref().to_ascii_uppercase())
                .unwrap_or_default();
            Ok(JsValue::new(boa_engine::string::JsString::from(tag)))
        });

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
            .accessor(
                boa_engine::string::JsString::from("innerHTML"),
                Some(get_inner_html), Some(set_inner_html), Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("textContent"),
                Some(get_text), Some(set_text), Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("className"),
                Some(get_class), Some(set_class), Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("id"),
                Some(get_id_attr), Some(set_id_attr), Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("value"),
                Some(get_value), Some(set_value), Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("parentNode"),
                Some(get_parent), None, Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("children"),
                Some(get_children), None, Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("firstElementChild"),
                Some(get_first_child), None, Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("nextElementSibling"),
                Some(get_next_sibling), None, Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("tagName"),
                Some(get_tag), None, Attribute::all(),
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

        
        // style: accessor per supported property, reading/writing the
        // element's inline style attribute (el.style.display = 'none' is
        // the other canonical mutation form besides class toggling).
        const STYLE_PROPS: &[(&str, &str)] = &[
            ("display", "display"), ("visibility", "visibility"),
            ("background", "background"), ("backgroundColor", "background-color"),
            ("backgroundImage", "background-image"), ("color", "color"),
            ("width", "width"), ("height", "height"),
            ("maxWidth", "max-width"), ("maxHeight", "max-height"),
            ("minWidth", "min-width"), ("minHeight", "min-height"),
            ("opacity", "opacity"), ("fontSize", "font-size"),
            ("fontWeight", "font-weight"), ("lineHeight", "line-height"),
            ("position", "position"), ("top", "top"), ("left", "left"),
            ("right", "right"), ("bottom", "bottom"),
            ("overflow", "overflow"), ("margin", "margin"),
            ("padding", "padding"), ("border", "border"),
            ("textAlign", "text-align"), ("cssFloat", "float"),
        ];
        let style_obj = ObjectInitializer::new(context)
            .property(boa_engine::string::JsString::from("__node_id"), doc_id, Attribute::all())
            .build();
        for (js_name, css_name) in STYLE_PROPS {
            let getter = boa_engine::object::FunctionObjectBuilder::new(
                context.realm(),
                NativeFunction::from_copy_closure(move |this, _args, ctx| {
                    let Some(n) = Engine::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
                    Ok(JsValue::new(boa_engine::string::JsString::from(Engine::get_style_prop(&n, css_name))))
                }),
            )
            .build();
            let setter = boa_engine::object::FunctionObjectBuilder::new(
                context.realm(),
                NativeFunction::from_copy_closure(move |this, args, ctx| {
                    let Some(n) = Engine::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
                    let v = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    Engine::set_style_prop(&n, css_name, &v);
                    Ok(JsValue::undefined())
                }),
            )
            .build();
            let _ = style_obj.define_property_or_throw(
                boa_engine::string::JsString::from(*js_name),
                boa_engine::property::PropertyDescriptor::builder()
                    .get(getter)
                    .set(setter)
                    .enumerable(true)
                    .configurable(true)
                    .build(),
                context,
            );
        }
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
