//! Aether's script lane (AETHERJS, LEDGER SR63): pages run on UnaOS's own ECMAScript engine, `js_core`
//! (JSCORE, SR55), and the DOM they see is a WebIDL binding over html_core's arena.
//!
//! Layout of the lane:
//!
//! - [`host`] — `js_core::vm::Host` for a page: console, the module loader (http_core / file), the clock,
//!   the time zone, entropy.
//! - [`idl`] — WebIDL plumbing: platform objects (branded `Internal` objects), interface objects and
//!   prototypes, operation/attribute/constant property attributes, argument conversion, DOMException.
//! - [`dom`] — Node, Document, DocumentFragment, DocumentType, CharacterData/Text/Comment/PI, Element,
//!   Attr/NamedNodeMap, NodeList/HTMLCollection (live, as proxies), DOMTokenList, DOMImplementation; the
//!   DOM Standard's mutation algorithms over the arena.
//! - [`html`] — HTMLElement and the per-element interfaces: reflected attributes, dataset, form-control
//!   values, HTMLMediaElement over `media`, the event-handler IDL attributes.
//! - [`cssom`] — CSSStyleDeclaration over css_core declarations, `getComputedStyle` from AETHERSTYLE's
//!   `computed_report`, `getBoundingClientRect`/`getClientRects`/offset metrics from the laid-out tree
//!   (AETHERINLINE's line fragments for inline boxes).
//! - [`events`] — EventTarget, Event and its subclasses, the DOM dispatch algorithm, inline event
//!   handlers, "report the exception".
//! - [`window`] — the global object (Window), Location, History, Navigator, Screen, timers,
//!   `requestAnimationFrame`, `queueMicrotask`, `matchMedia`, the platform preludes (fetch, URL, …).
//! - [`loader`] — HTML §4.12.1 script processing driven by the parser: parser-blocking, `defer`,
//!   `async` and module scripts, `document.write` during parse, dynamically inserted scripts, and
//!   §13.2.7 "the end" (DOMContentLoaded, load).
//!
//! Microtask checkpoints run where HTML puts them: after a script runs and after each callback the
//! event loop invokes ("clean up after running script" with an empty JavaScript execution context
//! stack), and after every task. Every entry from Rust into JavaScript is *guarded*: a panic poisons the
//! engine (no more script on this page; the DOM still renders) instead of ending the process, and a
//! per-task instruction budget turns a runaway script into a reported, uncatchable termination.

pub mod cssom;
pub mod dom;
pub mod events;
pub mod host;
pub mod html;
pub mod idl;
pub mod loader;
pub mod window;

use crate::dom::NodeRef;
use js_core::string::JsStr;
use js_core::vm::*;
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet, VecDeque};

// ---------------------------------------------------------------------------------------------------
// Engine poisoning: no page may kill the process
// ---------------------------------------------------------------------------------------------------

thread_local! {
    /// Set once a Rust panic has unwound out of running JS. Everything past that point refuses to run
    /// more script in this page.
    static POISONED: Cell<bool> = const { Cell::new(false) };
}

/// True once a panic has unwound out of this page's JS. The DOM built so far still renders.
pub fn engine_poisoned() -> bool {
    POISONED.with(Cell::get)
}

pub(crate) fn clear_poison() {
    POISONED.with(|p| p.set(false));
}

/// Truncates on a char boundary (every string reaching the ledger here came from a page).
pub(crate) fn clip(s: &str, n: usize) -> &str {
    let mut end = s.len().min(n);
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Runs one Rust → JS boundary with the process protected from a panic raised inside the engine or a
/// binding. js_core is fuzzed panic-free, but a binding bug (an arena borrow held across a call, say)
/// must cost the page its script, not the browser its process. After a catch the VM may be mid-frame,
/// so the page is poisoned: no further JS runs, the event is ledgered, the DOM renders as it stands.
pub fn guarded<T>(label: &str, f: impl FnOnce() -> T) -> Option<T> {
    if engine_poisoned() {
        return None;
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)) {
        Ok(v) => Some(v),
        Err(payload) => {
            POISONED.with(|p| p.set(true));
            let msg = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "<non-string panic>".to_string());
            crate::ledger::record_js(&format!("js-engine-poisoned:{}:{}", label, clip(&msg, 64)));
            None
        }
    }
}

// ---------------------------------------------------------------------------------------------------
// Page state
// ---------------------------------------------------------------------------------------------------

/// Instructions one task may run before it is terminated (≈ seconds of js_core time). A page's
/// `while (true) {}` ends here, reported, instead of holding the engine thread forever.
pub const TASK_BUDGET: u64 = 400_000_000;

thread_local! {
    static BUDGET: Cell<u64> = const { Cell::new(TASK_BUDGET) };
}

/// Sets the per-task instruction budget for this thread's pages (tests squeeze it).
pub fn set_task_budget(n: u64) {
    BUDGET.with(|b| b.set(n));
}

/// Who an event listener list belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TargetKey {
    Node(usize),
    Window,
    /// A `new EventTarget()` (or other non-node target), by its serial number.
    Other(u32),
}

/// One registered event listener (DOM §2.6). `callback` is a `host_roots` index.
#[derive(Clone, Debug)]
pub struct Listener {
    pub ty: String,
    pub callback: usize,
    pub capture: bool,
    pub once: bool,
    pub passive: bool,
    pub removed: bool,
    /// An event handler's internal listener (HTML §8.1.8.1): `callback` is unused; the handler value
    /// is looked up in `handlers` at invocation.
    pub handler: bool,
    pub serial: u64,
}

/// Per-document metadata for the documents a page creates (DOMParser, `createHTMLDocument`, …).
#[derive(Clone, Debug)]
pub struct DocMeta {
    pub html: bool,
    pub content_type: String,
    pub url: String,
    pub quirks: bool,
}

/// A task queued for the event loop (HTML §8.1.7): run in order by [`Engine::run_tasks`].
pub enum Task {
    /// A script element whose script is ready (dynamically inserted external scripts, async scripts).
    Script(usize),
    /// Fire a simple event at a target.
    Event(TargetKey, String, bool),
    /// Invoke a rooted callback with no arguments.
    Callback(usize),
}

#[derive(Default)]
pub(crate) struct PageState {
    /// The main document node (gives the arena; its id is `Document::ROOT`).
    pub doc: Option<NodeRef>,
    /// node id → (wrapper, root index). Wrappers live as long as the page: identity is stable.
    pub wrappers: HashMap<usize, (Obj, usize)>,
    pub ifaces: HashMap<&'static str, idl::Iface>,
    /// Set by mutating bindings; the engine consumes it to relayout.
    pub mutated: bool,
    /// Bumped on every DOM mutation (layout cache key).
    pub version: u64,
    pub url: String,
    /// The `<script>` element whose classic script is running.
    pub current_script: Option<usize>,
    /// JavaScript execution context stack depth as seen from the host (0 = empty).
    pub depth: u32,
    /// Script elements whose "already started" flag is set.
    pub started: HashSet<usize>,
    /// Script elements the parser created ("parser document" non-null).
    pub parser_inserted: HashSet<usize>,
    /// Node document for nodes not owned by the main document.
    pub node_doc: HashMap<usize, usize>,
    pub docs: HashMap<usize, DocMeta>,
    /// Attr objects: (element, attribute key) → (Attr, root).
    pub attrs: HashMap<(usize, String), (Obj, usize)>,
    pub listeners: HashMap<TargetKey, Vec<Listener>>,
    /// Event handler values (HTML §8.1.8.1): (target, "click") → host root of a function (or null).
    pub handlers: HashMap<(TargetKey, String), usize>,
    pub listener_serial: u64,
    pub target_serial: u32,
    pub tasks: VecDeque<Task>,
    /// requestAnimationFrame callbacks: (handle, root).
    pub raf: Vec<(u32, usize)>,
    pub raf_seq: u32,
    /// Form-control values with the dirty flag set (HTML §4.10.5.4), by element.
    pub values: HashMap<usize, String>,
    /// Checkedness of checkboxes/radios whose dirty checkedness flag is set.
    pub checked: HashMap<usize, bool>,
    /// Media element volume (0..1) and muted state set from script.
    pub media_volume: HashMap<usize, f64>,
    pub media_muted: HashMap<usize, bool>,
    /// The viewport layout reads (width, height) and the page's external stylesheets.
    pub viewport: (f32, f32),
    pub external_css: Vec<String>,
    pub scroll: (f64, f64),
    /// Laid-out tree cache keyed by `version`.
    pub layout: Option<(u64, crate::layout::LayoutTree)>,
    /// `document.activeElement`.
    pub focused: Option<usize>,
    /// Navigation a script staged (`location.href = …`, `location.assign`).
    pub pending_nav: Option<String>,
    /// Re-entrancy guard for "report the exception".
    pub reporting: bool,
    /// HTML "performing a microtask checkpoint" flag.
    pub in_checkpoint: bool,
    /// The readiness the loader has reached ("loading", "interactive", "complete").
    pub ready_state: &'static str,
    /// Uncaught errors seen (for diagnostics and tests).
    pub errors: Vec<String>,
    /// Pre-fetched external script sources by absolute URL.
    pub script_sources: HashMap<String, String>,
    pub inline_module_seq: u32,
    /// Console lines (level, text) — kept for tests and the headless report.
    pub console: Vec<(u8, String)>,
    /// Timers armed through `setTimeout` / `setInterval` (mirror of `vm.timers.len()`).
    pub timer_count: usize,
    /// [SameObject] platform objects per node: (node, which) → (object, root).
    pub same_object: HashMap<(usize, u8), (Obj, usize)>,
    /// Element namespace/prefix html_core's closed namespace set cannot hold (createElementNS).
    pub ns_override: HashMap<usize, (String, Option<String>)>,
    /// `new EventTarget()` objects by serial (rooted: listeners are keyed by the serial).
    pub other_targets: HashMap<u32, (Obj, usize)>,
    /// The page clock reading at the time origin (`performance.now()` = 0).
    pub time_origin: u64,
    /// Whether the last user click's event was canceled (`preventDefault`): no default action.
    pub last_click_canceled: bool,
    /// Script elements whose "force async" flag was cleared by an `async` IDL write.
    pub async_cleared: HashSet<usize>,
}

thread_local! {
    pub(crate) static PAGE: RefCell<PageState> = RefCell::new(PageState::default());
}

pub(crate) fn page<R>(f: impl FnOnce(&mut PageState) -> R) -> R {
    PAGE.with(|p| f(&mut p.borrow_mut()))
}

/// Marks the DOM mutated (relayout + layout-cache invalidation).
pub(crate) fn touch() {
    page(|p| {
        p.mutated = true;
        p.version = p.version.wrapping_add(1);
    });
}

/// True (and cleared) if the DOM was mutated by script since last asked.
pub fn take_mutated() -> bool {
    page(|p| std::mem::take(&mut p.mutated))
}

/// The current page's URL.
pub fn page_url() -> String {
    page(|p| p.url.clone())
}

/// Sets (or with `None` clears) the element `document.currentScript` reports.
pub fn set_current_script(node: Option<NodeRef>) {
    page(|p| p.current_script = node.map(|n| n.id().0));
}

/// The element a running classic script came from, if one is running.
pub fn current_script() -> Option<NodeRef> {
    let (doc, id) = page(|p| (p.doc.clone(), p.current_script));
    Some(doc?.node_at(html_core::NodeId(id?)))
}

/// Uncaught script errors this page reported (message strings).
pub fn reported_errors() -> Vec<String> {
    page(|p| p.errors.clone())
}

/// Console output of this page, in order.
pub fn console_lines() -> Vec<(u8, String)> {
    page(|p| p.console.clone())
}

/// A navigation the page's script requested (`location.href = …`), consumed.
pub fn take_script_navigation() -> Option<String> {
    page(|p| p.pending_nav.take())
}

/// The dirty form-control value of `node`, when script or the user set one (else the `value`
/// content attribute applies) — what rendering and form submission read.
pub fn control_value(node: &NodeRef) -> Option<String> {
    page(|p| p.values.get(&node.id().0).cloned())
}

/// Records a user edit of a text control's value (sets its dirty value).
pub fn set_control_value(node: &NodeRef, value: String) {
    page(|p| p.values.insert(node.id().0, value));
}

// ---------------------------------------------------------------------------------------------------
// Entering JavaScript
// ---------------------------------------------------------------------------------------------------

/// Runs `f` as (part of) a JavaScript execution: guarded against panics, and — when it is the outermost
/// entry — under a fresh instruction budget. Returns None when the page is poisoned or `f` panicked.
pub(crate) fn enter<T>(vm: &mut Vm, label: &str, f: impl FnOnce(&mut Vm) -> T) -> Option<T> {
    if engine_poisoned() {
        return None;
    }
    let outer = page(|p| {
        p.depth += 1;
        p.depth == 1
    });
    if outer {
        vm.budget = Some(BUDGET.with(Cell::get));
        vm.terminated = false;
    }
    let r = guarded(label, || f(vm));
    page(|p| p.depth = p.depth.saturating_sub(1));
    if outer {
        if vm.terminated {
            crate::ledger::record_js(&format!("js-task-terminated:{label}"));
            vm.terminated = false;
            vm.out_of_memory = false;
        }
        vm.budget = None;
    }
    r
}

/// True when no script is on the stack (HTML: the JavaScript execution context stack is empty).
pub(crate) fn stack_empty() -> bool {
    page(|p| p.depth == 0)
}

/// "Invoke a callback function" + "clean up after running script": calls `f`, reports an exception it
/// throws, and performs a microtask checkpoint if that emptied the stack. Returns the completion.
pub(crate) fn invoke(vm: &mut Vm, label: &str, f: &Value, this: &Value, args: &[Value]) -> Option<Value> {
    let r = enter(vm, label, |vm| vm.call(f, this, args));
    let out = match r {
        Some(Ok(v)) => Some(v),
        Some(Err(e)) => {
            events::report_exception(vm, &e);
            None
        }
        None => None,
    };
    if stack_empty() {
        checkpoint(vm);
    }
    out
}

/// Runs a classic script's source (ScriptEvaluation), reporting an uncaught exception, then the
/// microtask checkpoint when the stack is empty. Returns the completion value on success.
pub(crate) fn run_classic(vm: &mut Vm, src: &str, label: &str) -> Result<Value, Value> {
    let out = run_classic_no_checkpoint(vm, src, label);
    if stack_empty() {
        checkpoint(vm);
    }
    out
}

/// ScriptEvaluation + report, without the trailing checkpoint (the loader restores
/// `document.currentScript` first, as Blink does, so microtasks see `null`).
pub(crate) fn run_classic_no_checkpoint(vm: &mut Vm, src: &str, label: &str) -> Result<Value, Value> {
    let r = enter(vm, label, |vm| vm.run_script_str(src));
    match r {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => {
            events::report_exception(vm, &e);
            Err(e)
        }
        None => Err(Value::str("js engine poisoned")),
    }
}

/// A timer task's callback (with its extra arguments).
pub(crate) fn invoke_callback(vm: &mut Vm, label: &str, f: &Value, args: &[Value]) -> Option<Value> {
    invoke(vm, label, f, &Value::Undefined, args)
}

/// A string timer handler: compiled and run as a classic script.
pub(crate) fn run_timer_source(vm: &mut Vm, src: &str) -> Result<Value, Value> {
    run_classic(vm, src, "timer-source")
}

/// Whether the last user click was canceled by a listener (no default action follows).
pub fn last_click_canceled() -> bool {
    page(|p| p.last_click_canceled)
}

/// HTML §8.1.7.3 "perform a microtask checkpoint": run every queued microtask (promise jobs,
/// `queueMicrotask`), then notify about rejected promises nobody handled (§8.1.5.7).
pub(crate) fn checkpoint(vm: &mut Vm) {
    if engine_poisoned() || page(|p| std::mem::replace(&mut p.in_checkpoint, true)) {
        return;
    }
    let mut ran = 0usize;
    loop {
        let Some(job) = vm.jobs.pop_front() else { break };
        ran += 1;
        let r = enter(vm, "microtask", |vm| vm.run_job(job));
        match r {
            Some(Err(e)) => events::report_exception(vm, &e),
            Some(Ok(())) => {}
            None => break,
        }
        if ran >= 5_000_000 {
            crate::ledger::record_js("microtask-drain-cap");
            vm.jobs.clear();
            break;
        }
    }
    vm.kept_alive.clear();
    let pending: Vec<Obj> = std::mem::take(&mut vm.rejected_unhandled);
    for p in pending {
        let reason = match &vm.heap.get(p).kind {
            Kind::Promise(d) if !d.handled => Some(d.result.clone()),
            _ => None,
        };
        if let Some(reason) = reason {
            let msg = enter(vm, "rejection", |vm| vm.error_string(&reason)).unwrap_or_default();
            host::console_out(2, &format!("Uncaught (in promise) {msg}"));
            crate::ledger::record_js(&format!("unhandled-rejection:{}", clip(&msg, 64)));
        }
    }
    page(|p| p.in_checkpoint = false);
}

// ---------------------------------------------------------------------------------------------------
// The engine
// ---------------------------------------------------------------------------------------------------

/// One page's script engine: a js_core VM whose realm carries the page's Window.
pub struct Engine {
    pub vm: Vm,
}

impl Engine {
    /// A fresh realm bound to `document`'s arena (its main document node). Installs every interface;
    /// no page script runs.
    pub fn new(document: NodeRef) -> Engine {
        Self::with_viewport(document, 800.0, 600.0, Vec::new())
    }

    pub fn with_viewport(document: NodeRef, width: f32, height: f32, external_css: Vec<String>) -> Engine {
        // Reset the page state (the previous page's wrappers die with its VM).
        page(|p| {
            *p = PageState::default();
            p.doc = Some(document.document_node());
            p.viewport = (width, height);
            p.external_css = external_css;
            p.ready_state = "loading";
        });
        idl::reset();
        clear_poison();
        crate::api::fetch::reset_budget();
        crate::event_loop::reset();
        page(|p| p.time_origin = crate::event_loop::now_ms());
        let mut vm = Vm::new(Box::new(host::AetherHost::default()));
        vm.track_rejections = true;
        idl::install_domexception(&mut vm);
        events::install(&mut vm);
        dom::install(&mut vm);
        html::install(&mut vm);
        cssom::install(&mut vm);
        window::install(&mut vm);
        Engine { vm }
    }

    /// Evaluates one classic script in the page's realm (no `currentScript`). An uncaught exception is
    /// reported (window `error` event, console) and returned as `Err(message)`. A microtask checkpoint
    /// follows, as after any script.
    pub fn execute(&mut self, script: &str) -> Result<Value, String> {
        match run_classic(&mut self.vm, script, "script") {
            Ok(v) => Ok(v),
            Err(e) => Err(self.error_message(&e)),
        }
    }

    /// `execute`, with the completion value converted to a string.
    pub fn eval_string(&mut self, script: &str) -> Result<String, String> {
        let v = self.execute(script)?;
        let vm = &mut self.vm;
        match enter(vm, "to-string", |vm| vm.to_string(&v).map(|s| s.to_rust())) {
            Some(Ok(s)) => Ok(s),
            Some(Err(e)) => Err(self.error_message(&e)),
            None => Err("js engine poisoned".into()),
        }
    }

    /// `execute`, with the completion value converted to a number.
    pub fn eval_number(&mut self, script: &str) -> Result<f64, String> {
        let v = self.execute(script)?;
        let vm = &mut self.vm;
        match enter(vm, "to-number", |vm| vm.to_number(&v)) {
            Some(Ok(n)) => Ok(n),
            Some(Err(e)) => Err(self.error_message(&e)),
            None => Err("js engine poisoned".into()),
        }
    }

    pub fn error_message(&mut self, e: &Value) -> String {
        let vm = &mut self.vm;
        enter(vm, "error-string", |vm| vm.error_string(e)).unwrap_or_else(|| "js engine poisoned".to_string())
    }

    /// Points `location` and `document.URL` at `url` and the cookie accessor at its origin.
    pub fn set_location(&mut self, url: &str) {
        page(|p| {
            p.url = url.to_string();
            let root = p.doc.as_ref().map(|d| d.id().0).unwrap_or(0);
            p.docs.insert(root, DocMeta { html: true, content_type: "text/html".into(), url: url.to_string(), quirks: false });
        });
    }

    /// A microtask checkpoint.
    pub fn checkpoint(&mut self) {
        checkpoint(&mut self.vm);
    }

    /// Runs the queued tasks (dynamically inserted scripts, queued events), each followed by a
    /// microtask checkpoint. Only what is queued at entry runs: a task queued by a task waits for the
    /// next turn. Returns how many ran.
    pub fn run_tasks(&mut self) -> usize {
        let batch: Vec<Task> = page(|p| p.tasks.drain(..).collect());
        let mut n = 0;
        for t in batch {
            if engine_poisoned() {
                break;
            }
            n += 1;
            match t {
                Task::Script(id) => loader::execute_ready_script(&mut self.vm, id),
                Task::Event(target, ty, bubbles) => {
                    events::fire_simple(&mut self.vm, target, &ty, bubbles, false);
                }
                Task::Callback(r) => {
                    let f = idl::rooted(&self.vm, r);
                    idl::unroot(&mut self.vm, r);
                    invoke(&mut self.vm, "task", &f, &Value::Undefined, &[]);
                }
            }
            checkpoint(&mut self.vm);
        }
        n
    }

    /// One event-loop turn: queued tasks, then the timers due on the page clock (each its own task),
    /// each followed by a microtask checkpoint.
    pub fn tick(&mut self) -> usize {
        let mut n = self.run_tasks();
        n += crate::event_loop::fire_due_timers(&mut self.vm);
        checkpoint(&mut self.vm);
        n
    }

    /// Dispatches a trusted `click` (a MouseEvent that bubbles) at `node`, with the element's activation
    /// behavior. Returns true when a listener or handler ran.
    pub fn click(&mut self, node: &NodeRef) -> bool {
        events::user_click(&mut self.vm, node.id().0)
    }

    /// Fires a simple trusted event at `node`.
    pub fn fire(&mut self, node: &NodeRef, ty: &str, bubbles: bool) -> bool {
        events::fire_simple(&mut self.vm, TargetKey::Node(node.id().0), ty, bubbles, false)
    }

    /// Runs the queued `requestAnimationFrame` callbacks in bounded passes.
    pub fn drain_raf(&mut self) {
        window::drain_raf(&mut self.vm, 8);
    }

    /// One animation frame: the callbacks registered so far. Returns how many ran.
    pub fn drain_raf_once(&mut self) -> usize {
        let n = page(|p| p.raf.len());
        if n > 0 {
            window::run_raf_pass(&mut self.vm);
        }
        n
    }

    /// Whether tasks are queued.
    pub fn has_tasks(&self) -> bool {
        page(|p| !p.tasks.is_empty())
    }

    /// Records a same-document URL change (fragment navigation, `history.pushState`).
    pub(crate) fn set_doc_url(url: &str) {
        page(|p| {
            let root = p.doc.as_ref().map(|d| d.id().0).unwrap_or(0);
            if let Some(m) = p.docs.get_mut(&root) {
                m.url = url.to_string();
            }
        });
    }

    /// Updates the viewport geometry and scroll offset layout queries answer against.
    pub fn set_viewport(&mut self, width: f32, height: f32, scroll: (f64, f64)) {
        page(|p| {
            if p.viewport != (width, height) {
                p.layout = None;
            }
            p.viewport = (width, height);
            p.scroll = scroll;
        });
    }

    pub fn js_str(&self, s: &str) -> Value {
        Value::String(JsStr::from_str(s))
    }
}

/// Dispatches a trusted event named `event` at `node` (bubbling). Returns true if any listener or
/// handler ran.
pub fn dispatch_event(engine: &mut Engine, node: &NodeRef, event: &str) -> bool {
    if event == "click" {
        return engine.click(node);
    }
    engine.fire(node, event, true)
}

/// Runs the queued `requestAnimationFrame` callbacks in bounded passes.
pub fn drain_raf(engine: &mut Engine) {
    engine.drain_raf();
}
