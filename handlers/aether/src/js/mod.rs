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

thread_local! {
    /// URL of the page currently booted in this context. Plain Rust string
    /// (never a GC object), so native accessors — `document.cookie` — can
    /// reach it without holding anything that outlives the boa context.
    static PAGE_URL: RefCell<String> = const { RefCell::new(String::new()) };
}

/// The current page's URL, as last set by `Engine::set_location`.
pub fn page_url() -> String {
    PAGE_URL.with(|u| u.borrow().clone())
}

thread_local! {
    /// The `<script>` element whose source is executing right now, or `None`
    /// between scripts. A plain `NodeRef` (never a GC object), same
    /// discipline as `PAGE_URL`: the `document.currentScript` accessor wraps
    /// it on demand, so nothing here outlives the boa context.
    static CURRENT_SCRIPT: RefCell<Option<NodeRef>> = const { RefCell::new(None) };
}

/// Sets (or with `None` clears) the element `document.currentScript` reports.
/// The loader brackets each script's execution with this; everything that
/// runs later — timer/rAF drains, promise jobs, event callbacks — sees
/// `null`, which is what the spec requires of those contexts anyway.
pub fn set_current_script(node: Option<NodeRef>) {
    CURRENT_SCRIPT.with(|c| *c.borrow_mut() = node);
}

/// The element a running script came from, if one is running.
pub fn current_script() -> Option<NodeRef> {
    CURRENT_SCRIPT.with(|c| c.borrow().clone())
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

/// Builds a detached HTML-namespace element node directly — no parser, no
/// throwaway document, no selector round-trip (which silently failed for any
/// tag the HTML parser refuses to nest, and for custom elements).
///
/// The `QualName` is cloned off an element that already exists in the
/// document and retargeted at `local`, rather than written out with
/// `ns!(html)`: kuchiki 0.8 builds on html5ever 0.25, while this crate's own
/// `html5ever` dependency is 0.39, so the 0.39 `QualName`/`ns!` types are a
/// different type from the one `NodeRef::new_element` accepts and cannot be
/// passed to it. Cloning keeps the namespace exactly right by construction.
/// Any local name works, including unknown and custom tags.
pub(crate) fn new_html_element(local: &str) -> Option<NodeRef> {
    let doc = DOM_STATE.with(|s| s.borrow().document.clone())?;
    // First element in document order is <html> — html-namespaced.
    let mut name = doc
        .inclusive_descendants()
        .find_map(|n| n.as_element().map(|el| el.name.clone()))?;
    name.prefix = None;
    name.local = local.trim().to_ascii_lowercase().as_str().into();
    Some(NodeRef::new_element(
        name,
        std::iter::empty::<(kuchiki::ExpandedName, kuchiki::Attribute)>(),
    ))
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

/// `data-*` attribute name for a `dataset` property name, per the WHATWG
/// DOMStringMap mapping: every ASCII uppercase letter becomes `-` plus its
/// lowercase form, and the whole thing is prefixed `data-`.
///
/// `None` for the one shape the spec rejects — a `-` already followed by an
/// ASCII lowercase letter has no round trip back, so the setter throws
/// rather than writing an attribute it could never read again.
fn dataset_attr_name(prop: &str) -> Option<String> {
    let b = prop.as_bytes();
    for i in 0..b.len() {
        if b[i] == b'-' && b.get(i + 1).is_some_and(u8::is_ascii_lowercase) {
            return None;
        }
    }
    let mut out = String::from("data-");
    for c in prop.chars() {
        if c.is_ascii_uppercase() {
            out.push('-');
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    Some(out)
}

/// `dataset` property name for a `data-*` attribute — the inverse mapping:
/// `-` followed by an ASCII lowercase letter collapses to that letter
/// uppercased, any other `-` stays literal. `None` for attributes outside
/// the `data-` namespace.
fn dataset_prop_name(attr: &str) -> Option<String> {
    let rest = attr.strip_prefix("data-")?;
    let mut out = String::new();
    let mut it = rest.chars().peekable();
    while let Some(c) = it.next() {
        if c == '-' {
            if let Some(n) = it.peek().copied() {
                if n.is_ascii_lowercase() {
                    it.next();
                    out.push(n.to_ascii_uppercase());
                    continue;
                }
            }
        }
        out.push(c);
    }
    Some(out)
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
            // DOM interface constructors, as a real prototype chain rather
            // than three unrelated stubs. Bundles reach for these two ways:
            // `x instanceof HTMLAnchorElement` to branch, and
            // `window.HTMLDialogElement` / `'HTMLScriptElement' in window`
            // to feature-detect. A missing name is a ReferenceError that
            // kills the whole script, so the family is installed whole —
            // every interface a browser exposes for an HTML element, each
            // inheriting from HTMLElement -> Element -> Node -> EventTarget
            // exactly as the spec lays them out. They are not callable:
            // `new HTMLScriptElement()` throws "Illegal constructor", which
            // is what a browser does too.
            (function () {
                var mk = function (name, parent) {
                    var f = function () { throw new TypeError('Illegal constructor'); };
                    f.prototype = Object.create(parent ? parent.prototype : Object.prototype);
                    Object.defineProperty(f.prototype, 'constructor',
                        { value: f, writable: true, configurable: true });
                    try { Object.defineProperty(f, 'name', { value: name, configurable: true }); }
                    catch (e) {}
                    globalThis[name] = f;
                    return f;
                };
                var EventTarget = mk('EventTarget', null);
                var Node = mk('Node', EventTarget);
                var Element = mk('Element', Node);
                var HTMLElement = mk('HTMLElement', Element);
                mk('Document', Node);
                mk('DocumentFragment', Node);
                mk('ShadowRoot', globalThis.DocumentFragment);
                mk('DocumentType', Node);
                var CharacterData = mk('CharacterData', Node);
                mk('Text', CharacterData);
                mk('Comment', CharacterData);
                mk('CDATASection', globalThis.Text);
                mk('ProcessingInstruction', CharacterData);
                mk('Attr', Node);
                mk('SVGElement', Element);
                mk('HTMLDocument', globalThis.Document);
                // One interface per tag family, spelled the way the spec
                // spells it (the name is the observable thing here).
                var tags = [
                    'Anchor', 'Area', 'BR', 'Base', 'Body', 'Button',
                    'Canvas', 'DList', 'Data', 'DataList', 'Details', 'Dialog',
                    'Div', 'Embed', 'FieldSet', 'Font', 'Form', 'Frame',
                    'FrameSet', 'HR', 'Head', 'Heading', 'Html', 'IFrame',
                    'Image', 'Input', 'LI', 'Label', 'Legend', 'Link', 'Map',
                    'Marquee', 'Menu', 'Meta', 'Meter', 'Mod', 'OList',
                    'Object', 'OptGroup', 'Option', 'Output', 'Paragraph',
                    'Param', 'Picture', 'Pre', 'Progress', 'Quote', 'Script',
                    'Select', 'Slot', 'Source', 'Span', 'Style', 'TableCaption',
                    'TableCell', 'TableCol', 'TableRow', 'TableSection',
                    'Table', 'Template', 'TextArea', 'Time', 'Title', 'Track',
                    'UList', 'Unknown',
                ];
                for (var i = 0; i < tags.length; i++) {
                    mk('HTML' + tags[i] + 'Element', HTMLElement);
                }
                // <audio>/<video> hang off HTMLMediaElement, not straight
                // off HTMLElement — player code branches on exactly that.
                var HTMLMediaElement = mk('HTMLMediaElement', HTMLElement);
                mk('HTMLAudioElement', HTMLMediaElement);
                mk('HTMLVideoElement', HTMLMediaElement);
            })();
            globalThis.Image = function (w, h) {
                this.width = w || 0; this.height = h || 0;
                this.src = ''; this.complete = false;
                this.addEventListener = function () {};
            };
            var __mkStorage = function () {
                var m = {};
                return {
                    getItem: function (k) { return Object.prototype.hasOwnProperty.call(m, k) ? m[k] : null; },
                    setItem: function (k, v) { m[k] = String(v); },
                    removeItem: function (k) { delete m[k]; },
                    clear: function () { m = {}; },
                    key: function (i) { return Object.keys(m)[i] || null; },
                    get length() { return Object.keys(m).length; },
                };
            };
            globalThis.localStorage = __mkStorage();
            globalThis.sessionStorage = __mkStorage();
            globalThis.navigator = {
                userAgent: 'UnaOS Aether/0.1.0',
                language: 'en-US', languages: ['en-US'],
                platform: 'UnaOS', cookieEnabled: true,
                sendBeacon: function () { return true; },
            };
            globalThis.requestIdleCallback = function (cb) {
                return setTimeout(function () {
                    cb({ didTimeout: false, timeRemaining: function () { return 50; } });
                }, 0);
            };
            globalThis.cancelIdleCallback = function () {};
            "#,
        ));
        Self::setup_console(&mut context);
        // `__makeDataset` must exist before any node is wrapped — the
        // `dataset` accessor installed by `wrap_node` calls it — and
        // `setup_document` wraps the document immediately.
        Self::setup_dataset(&mut context);
        Self::setup_document(&mut context, document);
        crate::api::window::setup_window(&mut context);
        crate::api::events::init(&mut context);
        // window/global listeners route to the document, whose dispatch
        // path is real — load/DOMContentLoaded registrations land there.
        let _ = context.eval(boa_engine::Source::from_bytes(
            r#"
            globalThis.addEventListener = function (ev, cb) {
                document.addEventListener(ev, cb);
            };
            globalThis.removeEventListener = function () {};
            if (typeof window !== 'undefined' && window) {
                window.addEventListener = globalThis.addEventListener;
                window.removeEventListener = globalThis.removeEventListener;
            }
            "#,
        ));
        Self::setup_platform_breadth(&mut context);
        // URL/URLSearchParams, TextEncoder/Decoder, AbortController,
        // DOMParser and crypto. After breadth (and after `fetch` exists —
        // the abort shim wraps it) because it builds on both.
        crate::api::platform::init(&mut context);

        let mut engine = Self { context };
        engine.install_current_script_accessor();
        engine
    }

    /// `document.currentScript` as a real accessor over the loader's
    /// currently-executing `<script>` element: the wrapped element while a
    /// classic script runs, `null` otherwise (between scripts, and inside
    /// every callback/promise/timer continuation — our timer and rAF drains
    /// run after the script list, so that falls out for free).
    ///
    /// Webpack's chunk loader reads it to derive `__webpack_public_path__`
    /// and throws `InvariantError: Expected document.currentScript to be a
    /// <script> element` on null, which aborts the whole bundle boot.
    fn install_current_script_accessor(&mut self) {
        let getter = boa_engine::object::FunctionObjectBuilder::new(
            self.context.realm(),
            NativeFunction::from_fn_ptr(|_this, _args, ctx| match current_script() {
                Some(node) => {
                    crate::ledger::record_dom("document.currentScript:element");
                    Ok(Engine::wrap_node(ctx, node))
                }
                None => {
                    crate::ledger::record_dom("document.currentScript:null");
                    Ok(JsValue::null())
                }
            }),
        )
        .build();
        let Ok(doc) = self
            .context
            .global_object()
            .get(boa_engine::string::JsString::from("document"), &mut self.context)
        else {
            return;
        };
        let Some(doc) = doc.as_object() else { return };
        let _ = doc.define_property_or_throw(
            boa_engine::string::JsString::from("currentScript"),
            boa_engine::property::PropertyDescriptor::builder()
                .get(getter)
                .enumerable(true)
                .configurable(true)
                .build(),
            &mut self.context,
        );
    }

    /// Platform surface a bundled framework reaches for once its runtime
    /// actually boots: microtasks, the observer families, event
    /// constructors, base64, and `getComputedStyle`. Installed after
    /// `document` exists because several of these read it.
    ///
    /// Everything here is either the real semantics or an honest,
    /// self-reporting approximation — nothing invents a number. The
    /// observers register and never deliver, which is exactly what a
    /// browser looks like when no mutation, intersection or resize is
    /// pending; each registration is ledgered so a page that was waiting on
    /// one is visible rather than silently stalled.
    fn setup_platform_breadth(context: &mut Context) {
        let _ = context.eval(boa_engine::Source::from_bytes(
            r#"

            // Real microtask semantics — the promise job queue is the same
            // queue a browser drains, and the engine runs it at load.
            globalThis.queueMicrotask = function (cb) { Promise.resolve().then(cb); };

            // MessageChannel: real port wiring, delivery deferred through
            // the timer queue rather than its own task source. React's
            // scheduler yields through this; a missing MessageChannel sends
            // it down its setTimeout fallback, so honest wiring is closer.
            globalThis.MessageChannel = function () {
                var mk = function () {
                    return {
                        onmessage: null, _l: [],
                        addEventListener: function (ev, cb) { if (ev === 'message') this._l.push(cb); },
                        removeEventListener: function (ev, cb) {
                            if (ev === 'message') { var i = this._l.indexOf(cb); if (i >= 0) this._l.splice(i, 1); }
                        },
                        start: function () {}, close: function () {},
                    };
                };
                var a = mk(), b = mk();
                var wire = function (from, to) {
                    from.postMessage = function (data) {
                        setTimeout(function () {
                            var e = { data: data, type: 'message' };
                            if (to.onmessage) { try { to.onmessage(e); } catch (err) {} }
                            for (var i = 0; i < to._l.length; i++) { try { to._l[i](e); } catch (err) {} }
                        }, 0);
                    };
                };
                wire(a, b); wire(b, a);
                this.port1 = a; this.port2 = b;
            };

            globalThis.Event = function (type, init) {
                init = init || {};
                this.type = String(type);
                this.bubbles = !!init.bubbles;
                this.cancelable = !!init.cancelable;
                this.composed = !!init.composed;
                this.defaultPrevented = false;
                this.target = null; this.currentTarget = null;
                this.preventDefault = function () { this.defaultPrevented = true; };
                this.stopPropagation = function () {};
                this.stopImmediatePropagation = function () {};
            };
            globalThis.CustomEvent = function (type, init) {
                Event.call(this, type, init);
                this.detail = (init || {}).detail;
            };
            CustomEvent.prototype = Object.create(Event.prototype);

            // Observers: registration is honoured, delivery never fires.
            // One settled frame is rendered, so there is no mutation,
            // intersection or resize to report — reporting a fabricated one
            // would be worse than reporting none.
            var __mkObserver = function (kind) {
                return function (cb) {
                    this._cb = cb;
                    this.observe = function () { __ledger('observer-never-delivers:' + kind); };
                    this.unobserve = function () {};
                    this.disconnect = function () {};
                    this.takeRecords = function () { return []; };
                    this.root = null; this.rootMargin = '0px'; this.thresholds = [0];
                };
            };
            globalThis.MutationObserver = __mkObserver('MutationObserver');
            globalThis.IntersectionObserver = __mkObserver('IntersectionObserver');
            globalThis.ResizeObserver = __mkObserver('ResizeObserver');
            globalThis.PerformanceObserver = __mkObserver('PerformanceObserver');

            // Real base64 over Latin-1, exactly as the platform defines it.
            var __B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
            globalThis.btoa = function (input) {
                var s = String(input), out = '', i = 0;
                while (i < s.length) {
                    var c1 = s.charCodeAt(i++), c2 = s.charCodeAt(i++), c3 = s.charCodeAt(i++);
                    out += __B64.charAt(c1 >> 2);
                    out += __B64.charAt(((c1 & 3) << 4) | ((isNaN(c2) ? 0 : c2) >> 4));
                    out += isNaN(c2) ? '=' : __B64.charAt(((c2 & 15) << 2) | ((isNaN(c3) ? 0 : c3) >> 6));
                    out += isNaN(c3) ? '=' : __B64.charAt(c3 & 63);
                }
                return out;
            };
            globalThis.atob = function (input) {
                var s = String(input).replace(/[=\s]/g, ''), out = '', bits = 0, acc = 0;
                for (var i = 0; i < s.length; i++) {
                    var v = __B64.indexOf(s.charAt(i));
                    if (v < 0) continue;
                    acc = (acc << 6) | v; bits += 6;
                    if (bits >= 8) { bits -= 8; out += String.fromCharCode((acc >> bits) & 255); }
                }
                return out;
            };

            // structuredClone over the JSON-representable graph. Cycles,
            // Map/Set/Date/typed arrays are NOT preserved — the shortfall is
            // ledgered rather than papered over.
            globalThis.structuredClone = function (v) {
                try { return JSON.parse(JSON.stringify(v)); }
                catch (e) { __ledger('structuredClone-nonjson'); return v; }
            };

            // getComputedStyle: honest, and labelled as such. Values come
            // from the element's own inline style, falling back to the CSS
            // initial value for the properties pages branch on. The cascade
            // and layout live engine-side and are not consulted here, so a
            // property that was set by a stylesheet reads as its initial
            // value and the read is ledgered. Nothing here reports a
            // measured length — a fabricated pixel number is worse than an
            // admitted gap.
            var __INITIAL = {
                'display': 'block', 'visibility': 'visible', 'opacity': '1',
                'position': 'static', 'float': 'none', 'overflow': 'visible',
                'color': 'rgb(0, 0, 0)', 'background-color': 'rgba(0, 0, 0, 0)',
                'font-size': '16px', 'font-weight': '400', 'font-style': 'normal',
                'line-height': 'normal', 'text-align': 'start',
                'direction': 'ltr', 'text-transform': 'none',
                'z-index': 'auto', 'pointer-events': 'auto',
                'transform': 'none', 'animation-name': 'none',
                'margin': '0px', 'padding': '0px', 'border-width': '0px',
            };
            var __CAMEL = function (p) {
                return p.replace(/-([a-z])/g, function (_, c) { return c.toUpperCase(); });
            };
            globalThis.getComputedStyle = function (el) {
                var decls = {};
                var raw = (el && el.getAttribute) ? (el.getAttribute('style') || '') : '';
                var parts = String(raw).split(';');
                for (var i = 0; i < parts.length; i++) {
                    var j = parts[i].indexOf(':');
                    if (j > 0) {
                        decls[parts[i].slice(0, j).trim().toLowerCase()] = parts[i].slice(j + 1).trim();
                    }
                }
                var view = {
                    getPropertyValue: function (p) {
                        p = String(p).toLowerCase();
                        if (Object.prototype.hasOwnProperty.call(decls, p)) { return decls[p]; }
                        if (Object.prototype.hasOwnProperty.call(__INITIAL, p)) {
                            __ledger('getComputedStyle-initial-not-cascaded:' + p);
                            return __INITIAL[p];
                        }
                        __ledger('getComputedStyle-unknown:' + p);
                        return '';
                    },
                    getPropertyPriority: function () { return ''; },
                    setProperty: function (p, v) { decls[String(p).toLowerCase()] = v; },
                    removeProperty: function (p) { delete decls[String(p).toLowerCase()]; },
                    item: function (i) { return Object.keys(decls)[i] || ''; },
                };
                Object.defineProperty(view, 'length', { get: function () { return Object.keys(decls).length; } });
                var names = Object.keys(__INITIAL).concat(Object.keys(decls));
                for (var k = 0; k < names.length; k++) {
                    (function (p) {
                        var camel = __CAMEL(p);
                        var d = { configurable: true, enumerable: true,
                                  get: function () { return view.getPropertyValue(p); } };
                        try { Object.defineProperty(view, p, d); } catch (e) {}
                        if (camel !== p) { try { Object.defineProperty(view, camel, d); } catch (e) {} }
                    })(names[k]);
                }
                return view;
            };

            // Documents this engine runs are already fully parsed when the
            // first script executes; the engine advances readyState across
            // the lifecycle events it dispatches.
            //
            // `document.currentScript` is installed natively (see
            // install_current_script_accessor) — it reports the real
            // <script> element the loader is executing.
            if (typeof document !== 'undefined' && document) {
                document.readyState = 'loading';
                document.visibilityState = 'visible';
                document.hidden = false;
                document.referrer = '';
            }
            "#,
        ));
    }

    /// `element.dataset` — a real live `DOMStringMap` over the element's
    /// `data-*` attributes, not a snapshot object.
    ///
    /// It has to be live in both directions: bundles read a value the
    /// server rendered into the markup and then `delete` it (Next.js reads
    /// `documentElement.dataset.dplId` for its deployment id and removes it
    /// so a later hydration pass cannot see a stale one), and app shells
    /// write `dataset.*` as state that CSS attribute selectors then match.
    /// A plain object copy would serve the first read and silently lose
    /// every write, so the map is a `Proxy` whose traps go straight to the
    /// attribute list — the same `kuchiki` attributes selectors and
    /// `getAttribute` see. Nothing is cached; nothing is invented.
    fn setup_dataset(context: &mut Context) {
        fn node_of(args: &[JsValue], ctx: &mut Context) -> Option<NodeRef> {
            let id = args.first()?.to_number(ctx).ok()? as i32;
            DOM_STATE.with(|s| s.borrow().get_node(id))
        }
        fn key_of(args: &[JsValue], ctx: &mut Context) -> String {
            args.get(1)
                .cloned()
                .unwrap_or_default()
                .to_string(ctx)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default()
        }

        let get = NativeFunction::from_fn_ptr(|_this, args, ctx| {
            let (Some(node), key) = (node_of(args, ctx), key_of(args, ctx)) else {
                return Ok(JsValue::undefined());
            };
            let Some(attr) = dataset_attr_name(&key) else { return Ok(JsValue::undefined()) };
            let Some(el) = node.as_element() else { return Ok(JsValue::undefined()) };
            let v = el.attributes.borrow().get(attr.as_str()).map(str::to_string);
            Ok(match v {
                Some(v) => JsValue::new(boa_engine::string::JsString::from(v)),
                None => JsValue::undefined(),
            })
        });
        let set = NativeFunction::from_fn_ptr(|_this, args, ctx| {
            let (Some(node), key) = (node_of(args, ctx), key_of(args, ctx)) else {
                return Ok(JsValue::from(false));
            };
            let Some(attr) = dataset_attr_name(&key) else {
                // The spec throws SyntaxError here; ledger it and refuse the
                // write rather than storing an attribute that could never be
                // read back through the same map.
                crate::ledger::record_dom("dataset:invalid-name");
                return Ok(JsValue::from(false));
            };
            let value = args
                .get(2)
                .cloned()
                .unwrap_or_default()
                .to_string(ctx)
                .map(|s| s.to_std_string_escaped())
                .unwrap_or_default();
            let Some(el) = node.as_element() else { return Ok(JsValue::from(false)) };
            el.attributes.borrow_mut().insert(attr.as_str(), value);
            DOM_STATE.with(|s| s.borrow_mut().mutated = true);
            Ok(JsValue::from(true))
        });
        let del = NativeFunction::from_fn_ptr(|_this, args, ctx| {
            let (Some(node), key) = (node_of(args, ctx), key_of(args, ctx)) else {
                return Ok(JsValue::from(true));
            };
            let Some(attr) = dataset_attr_name(&key) else { return Ok(JsValue::from(true)) };
            if let Some(el) = node.as_element() {
                if el.attributes.borrow_mut().remove(attr.as_str()).is_some() {
                    DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                }
            }
            Ok(JsValue::from(true))
        });
        let keys = NativeFunction::from_fn_ptr(|_this, args, ctx| {
            let arr = boa_engine::object::builtins::JsArray::new(ctx);
            if let Some(node) = node_of(args, ctx) {
                if let Some(el) = node.as_element() {
                    let names: Vec<String> = el
                        .attributes
                        .borrow()
                        .map
                        .iter()
                        .filter(|(name, _)| name.ns.is_empty())
                        .filter_map(|(name, _)| dataset_prop_name(&name.local))
                        .collect();
                    for n in names {
                        let _ = arr.push(
                            JsValue::new(boa_engine::string::JsString::from(n)),
                            ctx,
                        );
                    }
                }
            }
            Ok(arr.into())
        });

        let _ = context.register_global_callable("__dataset_get".into(), 2, get);
        let _ = context.register_global_callable("__dataset_set".into(), 3, set);
        let _ = context.register_global_callable("__dataset_delete".into(), 2, del);
        let _ = context.register_global_callable("__dataset_keys".into(), 1, keys);

        let _ = context.eval(boa_engine::Source::from_bytes(
            r#"
            // DOMStringMap: every trap reaches the attribute list, so the
            // map is a view rather than a copy. Symbol keys are not data-*
            // names and are left to the (empty) target.
            globalThis.__makeDataset = function (id) {
                return new Proxy(Object.create(null), {
                    get: function (t, k) {
                        if (typeof k !== 'string') { return t[k]; }
                        return __dataset_get(id, k);
                    },
                    set: function (t, k, v) {
                        if (typeof k !== 'string') { t[k] = v; return true; }
                        return __dataset_set(id, k, v);
                    },
                    deleteProperty: function (t, k) {
                        if (typeof k !== 'string') { delete t[k]; return true; }
                        return __dataset_delete(id, k);
                    },
                    has: function (t, k) {
                        if (typeof k !== 'string') { return k in t; }
                        return __dataset_get(id, k) !== undefined;
                    },
                    ownKeys: function () { return __dataset_keys(id); },
                    getOwnPropertyDescriptor: function (t, k) {
                        if (typeof k !== 'string') {
                            return Object.getOwnPropertyDescriptor(t, k);
                        }
                        var v = __dataset_get(id, k);
                        if (v === undefined) { return undefined; }
                        return { value: v, writable: true, enumerable: true, configurable: true };
                    },
                });
            };
            "#,
        ));
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

        // __ledger(name): lets prelude-level shims record their own honest
        // failures in the same ledger the Rust bindings write to, so a stub
        // that answers without knowing shows up as a gap instead of passing
        // silently.
        let record = NativeFunction::from_fn_ptr(|_, args, ctx| {
            let name = args
                .get(0)
                .cloned()
                .unwrap_or_default()
                .to_string(ctx)
                .unwrap_or_default()
                .to_std_string_escaped();
            crate::ledger::record_js(&name[..name.len().min(64)]);
            Ok(JsValue::undefined())
        });
        context
            .register_global_callable(boa_engine::string::JsString::from("__ledger"), 1, record)
            .unwrap();

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
        let interface = Self::interface_name(&node);
        
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

        // dataset is built on demand rather than eagerly per wrapped node:
        // every node wrap would otherwise pay for a Proxy the page may never
        // touch. Non-elements have no attributes and so no dataset at all,
        // which is what a browser reports for the document and for text.
        let get_dataset = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::undefined()) };
            if n.as_element().is_none() {
                return Ok(JsValue::undefined());
            }
            let Some(id) = this
                .as_object()
                .and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok())
            else {
                return Ok(JsValue::undefined());
            };
            let make = ctx
                .global_object()
                .get(boa_engine::string::JsString::from("__makeDataset"), ctx)?;
            let Some(make) = make.as_callable() else {
                return Ok(JsValue::undefined());
            };
            make.call(&JsValue::undefined(), &[id], ctx)
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
        let get_parent_element = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::null()) };
            let mut cur = n.parent();
            while let Some(p) = cur {
                if p.as_element().is_some() {
                    return Ok(Self::wrap_node(ctx, p));
                }
                cur = p.parent();
            }
            Ok(JsValue::null())
        });
        let get_first_any_child = Self::native(context, |this, _args, ctx| {
            let Some(n) = Self::this_node(this, ctx) else { return Ok(JsValue::null()) };
            match n.first_child() {
                Some(c) => Ok(Self::wrap_node(ctx, c)),
                None => Ok(JsValue::null()),
            }
        });
        let get_child_nodes = Self::native(context, |this, _args, ctx| {
            let arr = boa_engine::object::builtins::JsArray::new(ctx);
            if let Some(n) = Self::this_node(this, ctx) {
                for child in n.children().take(512) {
                    let wrapped = Self::wrap_node(ctx, child);
                    let _ = arr.push(wrapped, ctx);
                }
            }
            Ok(arr.into())
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
                    match new_html_element(&tag) {
                        Some(node) => Ok(Self::wrap_node(ctx, node)),
                        None => Ok(JsValue::undefined()),
                    }
                }),
                boa_engine::string::JsString::from("createElement"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, args, ctx| {
                    let text = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    Ok(Self::wrap_node(ctx, kuchiki::NodeRef::new_text(text)))
                }),
                boa_engine::string::JsString::from("createTextNode"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, args, ctx| {
                    let text = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    Ok(Self::wrap_node(ctx, kuchiki::NodeRef::new_comment(text)))
                }),
                boa_engine::string::JsString::from("createComment"),
                1,
            )
            .function(
                // createElementNS: the namespace argument is accepted and
                // ignored — this engine's tree is HTML-namespaced, and an
                // SVG/MathML element built here still needs to be a real
                // node that appends, queries and serializes.
                NativeFunction::from_fn_ptr(|_, args, ctx| {
                    let tag = args.get(1).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    crate::ledger::record_dom("createElementNS-namespace-ignored");
                    match new_html_element(&tag) {
                        Some(node) => Ok(Self::wrap_node(ctx, node)),
                        None => Ok(JsValue::undefined()),
                    }
                }),
                boa_engine::string::JsString::from("createElementNS"),
                2,
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
                    // Removal is index-tombstoning: registrations are matched
                    // positionally with the JS __handlers array, so the pair
                    // (node, event) just stops matching.
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let event = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    DOM_STATE.with(|s| {
                        for (nid, ev) in s.borrow_mut().handlers.iter_mut() {
                            if *nid == id && *ev == event {
                                ev.clear(); // never matches a real event name again
                            }
                        }
                    });
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("removeEventListener"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let name = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let has = DOM_STATE.with(|s| s.borrow().get_node(id))
                        .and_then(|n| n.as_element().map(|el| el.attributes.borrow().get(name.as_str()).is_some()))
                        .unwrap_or(false);
                    Ok(JsValue::from(has))
                }),
                boa_engine::string::JsString::from("hasAttribute"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                boa_engine::string::JsString::from("focus"),
                0,
            )
            .function(
                NativeFunction::from_fn_ptr(|_, _, _| Ok(JsValue::undefined())),
                boa_engine::string::JsString::from("blur"),
                0,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, _args, ctx| {
                    // click(): dispatch to registered handlers.
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        dispatch_event(ctx, &n, "click");
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("click"),
                0,
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
                    let parent = DOM_STATE.with(|s| s.borrow().get_node(id));

                    let new_node_id = args.get(0).and_then(|v| v.as_object()).and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok()).and_then(|v| v.as_number()).unwrap_or(-1.0) as i32;
                    let new_node = DOM_STATE.with(|s| s.borrow().get_node(new_node_id));

                    let ref_arg = args.get(1);
                    let ref_node_id = if let Some(r) = ref_arg {
                        if r.is_null() || r.is_undefined() {
                            None
                        } else {
                            r.as_object().and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok()).and_then(|v| v.as_number()).map(|n| n as i32)
                        }
                    } else {
                        None
                    };

                    if let (Some(p), Some(n)) = (parent, new_node) {
                        if let Some(ref_id) = ref_node_id {
                            if let Some(r) = DOM_STATE.with(|s| s.borrow().get_node(ref_id)) {
                                r.insert_before(n);
                            }
                        } else {
                            p.append(n);
                        }
                        DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                    }
                    Ok(args.get(0).cloned().unwrap_or_default())
                }),
                boa_engine::string::JsString::from("insertBefore"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let parent = DOM_STATE.with(|s| s.borrow().get_node(id));

                    let new_node_id = args.get(0).and_then(|v| v.as_object()).and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok()).and_then(|v| v.as_number()).unwrap_or(-1.0) as i32;
                    let new_node = DOM_STATE.with(|s| s.borrow().get_node(new_node_id));

                    let old_node_id = args.get(1).and_then(|v| v.as_object()).and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok()).and_then(|v| v.as_number()).unwrap_or(-1.0) as i32;
                    let old_node = DOM_STATE.with(|s| s.borrow().get_node(old_node_id));

                    if let (Some(p), Some(n), Some(o)) = (parent, new_node, old_node) {
                        if o.parent().map_or(false, |cp| cp == p) {
                            o.insert_before(n.clone());
                            o.detach();
                            DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                        }
                    }
                    Ok(args.get(1).cloned().unwrap_or_default())
                }),
                boa_engine::string::JsString::from("replaceChild"),
                2,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        let html = n.to_string();
                        let frag = kuchiki::parse_html().one(html);
                        let cloned = if let Ok(mut bodies) = frag.select("body") {
                            if let Some(body) = bodies.next() {
                                if let Some(first_child) = body.as_node().children().next() {
                                    first_child.clone()
                                } else {
                                    return Ok(JsValue::undefined());
                                }
                            } else {
                                return Ok(JsValue::undefined());
                            }
                        } else {
                            return Ok(JsValue::undefined());
                        };

                        cloned.detach();

                        let deep = args.get(0).cloned().unwrap_or(JsValue::from(false)).to_boolean();
                        if !deep {
                            for child in cloned.children().collect::<Vec<_>>() {
                                child.detach();
                            }
                        }

                        return Ok(Self::wrap_node(ctx, cloned));
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("cloneNode"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let tag = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let arr = boa_engine::object::builtins::JsArray::new(ctx);
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        if let Ok(matches) = n.select(&tag) {
                            for m in matches.take(256) {
                                let wrapped = Self::wrap_node(ctx, m.as_node().clone());
                                let _ = arr.push(wrapped, ctx);
                            }
                        }
                    }
                    Ok(arr.into())
                }),
                boa_engine::string::JsString::from("getElementsByTagName"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let cls = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();
                    let arr = boa_engine::object::builtins::JsArray::new(ctx);
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        let selector = format!(".{}", cls);
                        if let Ok(matches) = n.select(&selector) {
                            for m in matches.take(256) {
                                let wrapped = Self::wrap_node(ctx, m.as_node().clone());
                                let _ = arr.push(wrapped, ctx);
                            }
                        }
                    }
                    Ok(arr.into())
                }),
                boa_engine::string::JsString::from("getElementsByClassName"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let other_id = args.get(0).and_then(|v| v.as_object()).and_then(|o| o.get(boa_engine::string::JsString::from("__node_id"), ctx).ok()).and_then(|v| v.as_number()).unwrap_or(-1.0) as i32;

                    let (this_node, other_node) = DOM_STATE.with(|s| {
                        let state = s.borrow();
                        (state.get_node(id), state.get_node(other_id))
                    });

                    let result = if let (Some(this), Some(other)) = (this_node, other_node) {
                        if this == other {
                            true
                        } else {
                            let mut cur = other.parent();
                            let mut found = false;
                            while let Some(p) = cur {
                                if p == this {
                                    found = true;
                                    break;
                                }
                                cur = p.parent();
                            }
                            found
                        }
                    } else {
                        false
                    };

                    Ok(JsValue::from(result))
                }),
                boa_engine::string::JsString::from("contains"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let selector = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();

                    let result = if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        if let Ok(mut matches) = n.select(&selector) {
                            matches.any(|m| m.as_node() == &n)
                        } else {
                            false
                        }
                    } else {
                        false
                    };

                    Ok(JsValue::from(result))
                }),
                boa_engine::string::JsString::from("matches"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let selector = args.get(0).cloned().unwrap_or_default().to_string(ctx).unwrap_or_default().to_std_string_escaped();

                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        let mut cur = Some(n);
                        while let Some(node) = cur {
                            if let Ok(mut matches) = node.select(&selector) {
                                if matches.any(|m| m.as_node() == &node) {
                                    return Ok(Self::wrap_node(ctx, node));
                                }
                            }
                            cur = node.parent();
                        }
                    }
                    Ok(JsValue::null())
                }),
                boa_engine::string::JsString::from("closest"),
                1,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, _args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                        n.detach();
                        DOM_STATE.with(|s| s.borrow_mut().mutated = true);
                    }
                    Ok(JsValue::undefined())
                }),
                boa_engine::string::JsString::from("remove"),
                0,
            )
            .function(
                NativeFunction::from_fn_ptr(|this, args, ctx| {
                    let id = this.as_object().unwrap().get(boa_engine::string::JsString::from("__node_id"), ctx).unwrap_or(JsValue::undefined()).as_number().unwrap_or(0.0) as i32;
                    let evt = args.get(0).cloned().unwrap_or_default();

                    if let Some(evt_obj) = evt.as_object() {
                        if let Ok(type_val) = evt_obj.get(boa_engine::string::JsString::from("type"), ctx) {
                            let event_type = type_val.to_string(ctx).unwrap_or_default().to_std_string_escaped();
                            if let Some(n) = DOM_STATE.with(|s| s.borrow().get_node(id)) {
                                dispatch_event(ctx, &n, &event_type);
                            }
                        }
                    }
                    Ok(JsValue::from(true))
                }),
                boa_engine::string::JsString::from("dispatchEvent"),
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
                boa_engine::string::JsString::from("parentElement"),
                Some(get_parent_element), None, Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("firstChild"),
                Some(get_first_any_child), None, Attribute::all(),
            )
            .accessor(
                boa_engine::string::JsString::from("childNodes"),
                Some(get_child_nodes), None, Attribute::all(),
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
            .accessor(
                boa_engine::string::JsString::from("dataset"),
                Some(get_dataset), None, Attribute::all(),
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

        // Wire the wrapper's prototype to the DOM interface its node
        // actually is, so `el instanceof HTMLAnchorElement`, `instanceof
        // HTMLElement` and `instanceof Node` all answer the way a browser
        // answers. Everything above is an OWN property of the wrapper, so
        // this only adds inheritance — no behavior already installed can be
        // shadowed by it.
        if let Some(proto) = Self::interface_prototype(context, interface) {
            js_node.set_prototype(Some(proto));
        }

        js_node.into()
    }

    /// The DOM interface a node presents, spelled the way the spec spells
    /// it. Element names map through the tag families the prelude installs;
    /// anything unrecognized is an unknown element, which is what a browser
    /// reports for it too.
    fn interface_name(node: &NodeRef) -> &'static str {
        if node.as_document().is_some() {
            return "HTMLDocument";
        }
        if node.as_text().is_some() {
            return "Text";
        }
        if node.as_comment().is_some() {
            return "Comment";
        }
        if node.as_doctype().is_some() {
            return "DocumentType";
        }
        let Some(el) = node.as_element() else { return "Node" };
        // Namespace decides first: an <a> or <title> inside <svg> is an
        // SVG element, not the HTML interface of the same name.
        if el.name.ns.as_ref() == "http://www.w3.org/2000/svg" {
            return "SVGElement";
        }
        Self::html_interface_for_tag(el.name.local.as_ref())
    }

    /// tag name -> HTML interface. One arm per family exactly as the spec
    /// groups them (h1..h6 share HTMLHeadingElement, td/th share
    /// HTMLTableCellElement, del/ins share HTMLModElement, and the long
    /// tail of semantic tags is plain HTMLElement).
    fn html_interface_for_tag(tag: &str) -> &'static str {
        match tag {
            "a" => "HTMLAnchorElement",
            "area" => "HTMLAreaElement",
            "audio" => "HTMLAudioElement",
            "base" => "HTMLBaseElement",
            "blockquote" | "q" => "HTMLQuoteElement",
            "body" => "HTMLBodyElement",
            "br" => "HTMLBRElement",
            "button" => "HTMLButtonElement",
            "canvas" => "HTMLCanvasElement",
            "caption" => "HTMLTableCaptionElement",
            "col" | "colgroup" => "HTMLTableColElement",
            "data" => "HTMLDataElement",
            "datalist" => "HTMLDataListElement",
            "del" | "ins" => "HTMLModElement",
            "details" => "HTMLDetailsElement",
            "dialog" => "HTMLDialogElement",
            "div" => "HTMLDivElement",
            "dl" => "HTMLDListElement",
            "embed" => "HTMLEmbedElement",
            "fieldset" => "HTMLFieldSetElement",
            "font" => "HTMLFontElement",
            "form" => "HTMLFormElement",
            "frame" => "HTMLFrameElement",
            "frameset" => "HTMLFrameSetElement",
            "h1" | "h2" | "h3" | "h4" | "h5" | "h6" => "HTMLHeadingElement",
            "head" => "HTMLHeadElement",
            "hr" => "HTMLHRElement",
            "html" => "HTMLHtmlElement",
            "iframe" => "HTMLIFrameElement",
            "img" => "HTMLImageElement",
            "input" => "HTMLInputElement",
            "label" => "HTMLLabelElement",
            "legend" => "HTMLLegendElement",
            "li" => "HTMLLIElement",
            "link" => "HTMLLinkElement",
            "map" => "HTMLMapElement",
            "marquee" => "HTMLMarqueeElement",
            "menu" => "HTMLMenuElement",
            "meta" => "HTMLMetaElement",
            "meter" => "HTMLMeterElement",
            "object" => "HTMLObjectElement",
            "ol" => "HTMLOListElement",
            "optgroup" => "HTMLOptGroupElement",
            "option" => "HTMLOptionElement",
            "output" => "HTMLOutputElement",
            "p" => "HTMLParagraphElement",
            "param" => "HTMLParamElement",
            "picture" => "HTMLPictureElement",
            "pre" | "listing" | "xmp" => "HTMLPreElement",
            "progress" => "HTMLProgressElement",
            "script" => "HTMLScriptElement",
            "select" => "HTMLSelectElement",
            "slot" => "HTMLSlotElement",
            "source" => "HTMLSourceElement",
            "span" => "HTMLSpanElement",
            "style" => "HTMLStyleElement",
            "table" => "HTMLTableElement",
            "tbody" | "thead" | "tfoot" => "HTMLTableSectionElement",
            "td" | "th" => "HTMLTableCellElement",
            "template" => "HTMLTemplateElement",
            "textarea" => "HTMLTextAreaElement",
            "time" => "HTMLTimeElement",
            "title" => "HTMLTitleElement",
            "tr" => "HTMLTableRowElement",
            "track" => "HTMLTrackElement",
            "ul" => "HTMLUListElement",
            "video" => "HTMLVideoElement",
            // The semantic long tail — abbr, article, section, strong, and
            // every custom `<my-widget>` a framework defines — is plain
            // HTMLElement in a browser, not an unknown element.
            "abbr" | "address" | "article" | "aside" | "b" | "bdi" | "bdo" | "cite" | "code"
            | "dd" | "dfn" | "dt" | "em" | "figcaption" | "figure" | "footer" | "header"
            | "hgroup" | "i" | "kbd" | "main" | "mark" | "nav" | "noscript" | "rp" | "rt"
            | "ruby" | "s" | "samp" | "search" | "section" | "small" | "strong" | "sub"
            | "summary" | "sup" | "u" | "var" | "wbr" => "HTMLElement",
            // Hyphenated names are custom elements, which are HTMLElement.
            other if other.contains('-') => "HTMLElement",
            _ => "HTMLUnknownElement",
        }
    }

    /// `globalThis[name].prototype`, falling back through
    /// HTMLUnknownElement to HTMLElement if the realm lacks the interface.
    fn interface_prototype(
        context: &mut Context,
        name: &str,
    ) -> Option<boa_engine::JsObject> {
        for candidate in [name, "HTMLUnknownElement", "HTMLElement"] {
            let global = context.global_object();
            let Ok(ctor) = global.get(boa_engine::string::JsString::from(candidate), context) else {
                continue;
            };
            let Some(ctor) = ctor.as_object() else { continue };
            let Ok(proto) = ctor.get(boa_engine::string::JsString::from("prototype"), context)
            else {
                continue;
            };
            if let Some(proto) = proto.as_object() {
                return Some(proto);
            }
        }
        None
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
    
    /// Installs `document.cookie` as a real accessor pair over the session
    /// cookie jar: the getter formats what the jar would send to the current
    /// page URL, the setter feeds one `"name=value; attrs"` string back in.
    /// Same jar the network stack uses, so a cookie script-set here rides
    /// the next fetch, and a `Set-Cookie` from the wire is readable here.
    fn install_cookie_accessor(&mut self) {
        let getter = boa_engine::object::FunctionObjectBuilder::new(
            self.context.realm(),
            NativeFunction::from_fn_ptr(|_this, _args, _ctx| {
                crate::ledger::record_dom("document.cookie:get");
                Ok(JsValue::new(boa_engine::string::JsString::from(
                    crate::net::cookies_for(&page_url()),
                )))
            }),
        )
        .build();
        let setter = boa_engine::object::FunctionObjectBuilder::new(
            self.context.realm(),
            NativeFunction::from_fn_ptr(|_this, args, ctx| {
                let decl = args
                    .get(0)
                    .cloned()
                    .unwrap_or_default()
                    .to_string(ctx)
                    .unwrap_or_default()
                    .to_std_string_escaped();
                if !decl.trim().is_empty() {
                    crate::net::set_cookie(&page_url(), &decl);
                    crate::ledger::record_dom("document.cookie:set");
                }
                Ok(JsValue::undefined())
            }),
        )
        .build();
        let Ok(doc) = self
            .context
            .global_object()
            .get(boa_engine::string::JsString::from("document"), &mut self.context)
        else {
            return;
        };
        let Some(doc) = doc.as_object() else { return };
        let _ = doc.define_property_or_throw(
            boa_engine::string::JsString::from("cookie"),
            boa_engine::property::PropertyDescriptor::builder()
                .get(getter)
                .set(setter)
                .enumerable(true)
                .configurable(true)
                .build(),
            &mut self.context,
        );
    }

    /// Points window.location at the loaded page and wires document.cookie
    /// to the session jar for that origin. Plain data properties — enough
    /// for the hostname/pathname branching real scripts do at boot.
    pub fn set_location(&mut self, url: &str) {
        PAGE_URL.with(|u| *u.borrow_mut() = url.to_string());
        self.install_cookie_accessor();
        let parsed = url::Url::parse(url).ok();
        let host = parsed.as_ref().and_then(|u| u.host_str()).unwrap_or("");
        let path = parsed.as_ref().map(|u| u.path()).unwrap_or("/");
        let protocol = parsed
            .as_ref()
            .map(|u| format!("{}:", u.scheme()))
            .unwrap_or_else(|| "https:".to_string());
        let search = parsed
            .as_ref()
            .and_then(|u| u.query())
            .map(|q| format!("?{}", q))
            .unwrap_or_default();
        let origin = parsed
            .as_ref()
            .map(|u| format!("{}//{}", protocol, u.host_str().unwrap_or("")))
            .unwrap_or_default();
        let esc = |s: &str| s.replace('\\', "").replace('\'', "");
        let script = format!(
            r#"
            if (typeof location !== 'undefined' && location) {{
                location.href = '{href}';
                location.hostname = '{host}';
                location.host = '{host}';
                location.pathname = '{path}';
                location.protocol = '{protocol}';
                location.search = '{search}';
                location.hash = '';
                location.origin = '{origin}';
                if (typeof window !== 'undefined' && window) {{ window.location = location; }}
            }}
            "#,
            href = esc(url),
            host = esc(host),
            path = esc(path),
            protocol = esc(&protocol),
            search = esc(&search),
            origin = esc(&origin),
        );
        let _ = self.execute(&script);
    }

    pub fn execute(&mut self, script: &str) -> JsResult<JsValue> {
        use boa_engine::Source;
        self.context.eval(Source::from_bytes(script))
    }
}
