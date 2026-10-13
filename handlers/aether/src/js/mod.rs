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
//! - [`range`] — AbstractRange/StaticRange/Range and the live-range mutation rules (AETHERWPT, SR72).
//! - [`traversal`] — NodeFilter, NodeIterator, TreeWalker (SR72).
//! - [`reflect`] — HTML §2.6.1 reflection of every reflected IDL attribute (SR72).
//! - [`controls`] — form controls' DOM half: value modes, selection, constraint validation (SR72).
//! - [`forms`] — the entry list and `formdata`, FormData/File, script-started submission navigation,
//!   the form and select legacy platform objects (AETHERFORMS, SR79).
//! - [`shadow`], [`mutation`], [`custom`] — shadow trees, MutationObserver and custom elements over
//!   `dom::shadow` / `dom::mutation` / `dom::custom` (AETHERSHADOW, SR82); `mutation::settle` is the
//!   [CEReactions] / observer-microtask boundary the mutation algorithms end with.
//! - [`canvas`] — the 2D canvas (AETHERCANVAS, SR85): CanvasRenderingContext2D, Path2D, gradients,
//!   patterns, ImageData, TextMetrics, OffscreenCanvas, ImageBitmap, DOMMatrix, over `crate::canvas`.
//! - [`broadcast`], [`idb`], [`cache_api`], [`locks`], [`sw`] — the storage family (AETHERSTORAGE, SR94): the
//!   cross-document inbox, BroadcastChannel and structured serialization; IndexedDB and `navigator.storage`;
//!   the Cache API; Web Locks; `navigator.serviceWorker` — over `crate::storage` and `crate::workers`.
//! - [`loader`] — HTML §4.12.1 script processing driven by the parser: parser-blocking, `defer`,
//!   `async` and module scripts, `document.write` during parse, dynamically inserted scripts, and
//!   §13.2.7 "the end" (DOMContentLoaded, load).
//!
//! Microtask checkpoints run where HTML puts them: after a script runs and after each callback the
//! event loop invokes ("clean up after running script" with an empty JavaScript execution context
//! stack), and after every task. Every entry from Rust into JavaScript is *guarded*: a panic poisons the
//! engine (no more script on this page; the DOM still renders) instead of ending the process, and a
//! per-task instruction budget turns a runaway script into a reported, uncatchable termination.

// AETHERSTYLE3B (SR115): the Web Animations API and the typed transition / animation events.
pub mod animations;
pub mod broadcast;
pub mod cache_api;
pub mod canvas;
pub mod controls;
pub mod cssom;
pub mod custom;
pub mod dom;
pub mod events;
pub mod forms;
pub mod host;
pub mod html;
pub mod idb;
pub mod idl;
pub mod importmap;
pub mod loader;
pub mod locks;
pub mod mutation;
pub mod ports;
pub mod range;
pub mod reflect;
mod reflect_table;
pub mod shadow;
pub mod sw;
pub mod traversal;
pub mod net;
pub mod window;
pub mod worker;

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
    /// HTML §8.1.5.7 "notify about rejected promises": fire `unhandledrejection` for the rooted promise.
    Rejection(usize),
    /// A reported promise was handled later: fire `rejectionhandled` for it.
    RejectionHandled(usize),
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
    /// HTML "appropriate template contents owner document" per document (AETHERDEBT SR93): document →
    /// its inert template contents owner.
    pub template_owner: HashMap<usize, usize>,
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
    /// Handles canceled while their frame's callbacks are running (HTML "run the animation frame
    /// callbacks" skips a callback an earlier one of the same frame canceled).
    pub raf_cancelled: HashSet<u32>,
    /// HTML's outstanding rejected promises weak set: promises whose `unhandledrejection` fired and that
    /// are still unhandled, rooted until handled (`rejectionhandled`) or the page goes.
    pub outstanding_rejections: Vec<(Obj, usize)>,
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
    /// A POST form submission a script started (`submit()`, `requestSubmit()`), for the shell.
    pub pending_submit: Option<crate::forms::OpenDocument>,
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
    /// The timer nesting level of the running timer task (None outside timer tasks).
    pub timer_nesting: Option<u32>,
    /// Each armed timer's nesting level, by id.
    pub timer_levels: HashMap<u32, u32>,
    /// JSPOS (SR127): the source of each classic page script run, by the file name its frames carry (the
    /// latest per name; [`RAN_SOURCES_CAP`] bytes at most), for the console's excerpt under an uncaught error.
    pub ran_sources: HashMap<String, std::rc::Rc<str>>,
    /// JSPOS: console index of an uncaught error's line -> its frames and source excerpt (the devtools
    /// Console prints them under the line; `console_lines` stays the one-line form).
    pub console_details: HashMap<usize, String>,
}

/// Bytes of page-script source kept for console excerpts.
const RAN_SOURCES_CAP: usize = 8 << 20;

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

/// JSPOS (SR127): the console output with each uncaught error's frames (`    at fn (url:line:col)`) and the
/// source excerpt around its position joined under its line — what the devtools Console shows.
pub fn console_lines_detailed() -> Vec<(u8, String)> {
    page(|p| {
        p.console
            .iter()
            .enumerate()
            .map(|(i, (l, t))| match p.console_details.get(&i) {
                Some(d) => (*l, format!("{t}\n{d}")),
                None => (*l, t.clone()),
            })
            .collect()
    })
}

/// JSPOS: the (filename, lineno, colno) an ErrorEvent for `err` carries — the position the error recorded
/// when it was made (vm/ops.rs `error_position`: the innermost frame with a line, or a SyntaxError's
/// offending token); else the page URL and 0, 0 (a thrown non-error value: no position is recorded).
pub fn error_site(vm: &Vm, err: &Value) -> (String, u32, u32) {
    match vm.error_position(err) {
        Some(p) => (p.file, p.line, p.col),
        None => (page_url(), 0, 0),
    }
}

/// The file name a classic script's frames carry: the running `<script src>`'s absolute URL, else the
/// document's URL (inline scripts, timer strings, `javascript:` URLs, the console evaluator).
fn script_file() -> String {
    if let Some(el) = page(|p| p.current_script) {
        if let Some(src) = dom::attr_value(el, "src") {
            if let Some(u) = dom::resolve_url(dom::node_document(el), &src) {
                return u;
            }
        }
    }
    page_url()
}

/// AETHERDEVTOOLS3 (SR128): the devtools Sources panel's goto hook — the source the engine ran under the
/// frame file name `file` (the latest per name, as the console excerpt reads it), else the external script
/// the loader pre-fetched under that absolute URL. None when it holds neither.
pub fn ran_source(file: &str) -> Option<std::rc::Rc<str>> {
    page(|p| p.ran_sources.get(file).cloned().or_else(|| p.script_sources.get(file).map(|s| std::rc::Rc::from(s.as_str()))))
}

/// Keeps `src` as the source of `file` for console excerpts (bounded by [`RAN_SOURCES_CAP`]).
fn remember_source(file: &str, src: &str) {
    page(|p| {
        let held: usize = p.ran_sources.iter().filter(|(k, _)| k.as_str() != file).map(|(_, v)| v.len()).sum();
        if held + src.len() <= RAN_SOURCES_CAP {
            p.ran_sources.insert(file.to_string(), std::rc::Rc::from(src));
        }
    });
}

/// "Report the exception" (js/events.rs `report_exception`) and then, when it reached the console (nobody
/// canceled the `error` event), the error's frames and source excerpt recorded under that console line.
/// `running` is the (file, source) of the script that was running, when the caller has it.
pub(crate) fn report_uncaught(vm: &mut Vm, err: &Value, running: Option<(&str, &str)>) {
    let before = page(|p| p.console.len());
    events::report_exception(vm, err);
    note_uncaught_detail(vm, err, before, running);
}

/// Records the frames and source excerpt of `err` under the first `Uncaught …` console line at or after
/// index `before` (once per line).
pub(crate) fn note_uncaught_detail(vm: &Vm, err: &Value, before: usize, running: Option<(&str, &str)>) {
    let at = page(|p| (before..p.console.len()).find(|&i| p.console[i].0 == 2 && p.console[i].1.starts_with("Uncaught ") && !p.console_details.contains_key(&i)));
    let Some(at) = at else { return };
    let mut detail = vm.error_frames(err).unwrap_or_default();
    if let Some(pos) = vm.error_position(err) {
        let src: Option<std::rc::Rc<str>> = match running {
            Some((f, s)) if f == pos.file => Some(std::rc::Rc::from(s)),
            _ => page(|p| p.ran_sources.get(&pos.file).cloned()),
        };
        let ex = src.map(|s| source_excerpt(&s, pos.line, pos.col)).unwrap_or_default();
        if !ex.is_empty() {
            if !detail.is_empty() {
                detail.push('\n');
            }
            detail.push_str(&ex);
        }
    }
    if !detail.is_empty() {
        page(|p| p.console_details.insert(at, detail));
    }
}

/// The Sources-style excerpt around `line` (1-based) of `src`: two lines either side, numbered, the line
/// marked `>`, and a caret under UTF-16 column `col`. Lines split where js_core's line table splits them
/// (LF, CR, CR LF, U+2028, U+2029). Empty when the line is past the end.
pub fn source_excerpt(src: &str, line: u32, col: u32) -> String {
    let mut lines: Vec<&str> = Vec::new();
    let mut start = 0;
    let mut it = src.char_indices().peekable();
    while let Some((i, c)) = it.next() {
        if matches!(c, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
            lines.push(&src[start..i]);
            start = i + c.len_utf8();
            if c == '\r' && it.peek().map(|&(_, n)| n) == Some('\n') {
                it.next();
                start += 1;
            }
        }
    }
    lines.push(&src[start..]);
    let l = line as usize;
    if l == 0 || l > lines.len() {
        return String::new();
    }
    let (first, last) = (l.saturating_sub(2).max(1), (l + 2).min(lines.len()));
    let w = last.to_string().len();
    let mut out = String::new();
    for n in first..=last {
        if !out.is_empty() {
            out.push('\n');
        }
        let mark = if n == l { '>' } else { ' ' };
        out.push_str(&format!("{mark} {n:>w$} | {}", lines[n - 1]));
        if n == l {
            let mut pad = String::new();
            let mut units = 0u32;
            for c in lines[n - 1].chars() {
                if units + 1 >= col {
                    break;
                }
                units += c.len_utf16() as u32;
                pad.push(if c == '\t' { '\t' } else { ' ' });
            }
            out.push_str(&format!("\n  {:>w$} | {pad}^", ""));
        }
    }
    out
}

/// A navigation the page's script requested (`location.href = …`), consumed.
pub fn take_script_navigation() -> Option<String> {
    page(|p| p.pending_nav.take())
}

/// A POST form submission the page's script started (method, body and enctype from
/// `crate::forms::plan_navigation`), consumed. Script-started GET submissions arrive through
/// [`take_script_navigation`].
pub fn take_form_submission() -> Option<crate::forms::OpenDocument> {
    page(|p| p.pending_submit.take())
}

/// Whether `node` belongs to the page whose script state this thread holds (layout and tests also
/// run over documents no script ever saw).
fn is_page_node(p: &PageState, node: &NodeRef) -> bool {
    p.doc.as_ref().is_some_and(|d| d.same_document(node))
}

/// The dirty form-control value of `node`, when script or the user set one (else the `value`
/// content attribute applies) — what rendering and form submission read.
pub fn control_value(node: &NodeRef) -> Option<String> {
    page(|p| if is_page_node(p, node) { p.values.get(&node.id().0).cloned() } else { None })
}

/// Script-set selectedness of an `<option>` (`option.selected`, `select.value`), if any.
pub fn option_selectedness(node: &NodeRef) -> Option<bool> {
    page(|p| if is_page_node(p, node) { p.checked.get(&node.id().0).copied() } else { None })
}

/// Records a user edit of a text control's value (sets its dirty value).
pub fn set_control_value(node: &NodeRef, value: String) {
    page(|p| {
        if is_page_node(p, node) {
            p.values.insert(node.id().0, value);
        }
    });
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
            report_uncaught(vm, &e, None);
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
///
/// JSPOS (SR127): the script runs under its file name (`script_file`), so its frames and a SyntaxError
/// name `url:line:col`; a page script's source is kept for the console's excerpt.
pub(crate) fn run_classic_no_checkpoint(vm: &mut Vm, src: &str, label: &str) -> Result<Value, Value> {
    let file = script_file();
    if label == "script" {
        remember_source(&file, src);
    }
    let r = enter(vm, label, |vm| vm.run_script_str_at(src, &file));
    match r {
        Some(Ok(v)) => Ok(v),
        Some(Err(e)) => {
            report_uncaught(vm, &e, Some((&file, src)));
            Err(e)
        }
        None => Err(Value::str("js engine poisoned")),
    }
}

/// A timer task's callback (with its extra arguments).
pub(crate) fn invoke_callback(vm: &mut Vm, label: &str, f: &Value, args: &[Value]) -> Option<Value> {
    invoke(vm, label, f, &Value::Undefined, args)
}

/// A string timer handler: compiled and run as a classic script — after HostEnsureCanCompileStrings
/// (HTML §8.6 timer initialization steps, AETHERDEBT SR93): under a CSP without `'unsafe-eval'` the
/// EvalError is reported for the global and the handler does not run.
pub(crate) fn run_timer_source(vm: &mut Vm, src: &str) -> Result<Value, Value> {
    if let Err(m) = crate::security::compile_strings_allowed(src) {
        let p = vm.intr().eval_error_proto;
        let e = vm.make_error(p, &m);
        report_uncaught(vm, &e, None);
        return Err(e);
    }
    run_classic(vm, src, "timer-source")
}

/// The nesting level a firing timer's task runs at (`keep`: an interval stays armed, so its level
/// record stays too).
pub(crate) fn timer_level(id: u32, keep: bool) -> u32 {
    page(|p| if keep { p.timer_levels.get(&id).copied() } else { p.timer_levels.remove(&id) }).unwrap_or(1)
}

pub(crate) fn set_timer_nesting(level: Option<u32>) {
    page(|p| p.timer_nesting = level);
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
    // AETHERSHADOW: an observer microtask or custom element reactions a mutation owed without a
    // settle of its own (a removal outside the binding's algorithms) are taken up here.
    mutation::settle(vm);
    let mut ran = 0usize;
    loop {
        let Some(job) = vm.jobs.pop_front() else { break };
        ran += 1;
        let r = enter(vm, "microtask", |vm| vm.run_job(job));
        match r {
            Some(Err(e)) => report_uncaught(vm, &e, None),
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
    // HTML §8.1.5.7 "notify about rejected promises": each promise still unhandled at the end of the
    // checkpoint gets a task (DOM manipulation task source) that fires a cancelable `unhandledrejection`
    // at the Window; only an uncanceled one reaches the console (`fire_rejection`).
    let pending: Vec<Obj> = std::mem::take(&mut vm.rejected_unhandled);
    for p in pending {
        let unhandled = matches!(&vm.heap.get(p).kind, Kind::Promise(d) if !d.handled);
        if unhandled {
            let r = idl::root(vm, Value::Object(p));
            page(|pg| pg.tasks.push_back(Task::Rejection(r)));
        }
    }
    page(|p| p.in_checkpoint = false);
}

/// The `unhandledrejection` (or, `handled_event`, the `rejectionhandled`) task for the rooted promise `r`
/// (HTML §8.1.5.7): a PromiseRejectionEvent {promise, reason} at the Window. An `unhandledrejection` nobody
/// canceled is reported to the console; a promise still unhandled after it joins the outstanding set.
pub(crate) fn fire_rejection(vm: &mut Vm, r: usize, handled_event: bool) {
    let Value::Object(p) = idl::rooted(vm, r) else {
        idl::unroot(vm, r);
        return;
    };
    let (is_handled, reason) = match &vm.heap.get(p).kind {
        Kind::Promise(d) => (d.handled, d.result.clone()),
        _ => (true, Value::Undefined),
    };
    if !handled_event && is_handled {
        idl::unroot(vm, r);
        return;
    }
    let mut not_canceled = true;
    if !engine_poisoned() {
        let init = vm.new_plain_object();
        let _ = vm.create_data_property(init, PropertyKey::from_str("promise"), Value::Object(p));
        let _ = vm.create_data_property(init, PropertyKey::from_str("reason"), reason.clone());
        let ty = if handled_event { "rejectionhandled" } else { "unhandledrejection" };
        let e = events::new_event(vm, "PromiseRejectionEvent", ty, false, !handled_event, Some(init));
        let er = idl::root(vm, Value::Object(e));
        not_canceled = events::dispatch(vm, e, TargetKey::Window).0;
        idl::unroot(vm, er);
    }
    if handled_event {
        idl::unroot(vm, r);
        return;
    }
    if not_canceled {
        let msg = enter(vm, "rejection", |vm| vm.error_string(&reason)).unwrap_or_default();
        host::console_out(2, &format!("Uncaught (in promise) {msg}"));
        crate::ledger::record_js(&format!("unhandled-rejection:{}", clip(&msg, 64)));
    }
    let still = matches!(&vm.heap.get(p).kind, Kind::Promise(d) if !d.handled);
    if still {
        page(|pg| pg.outstanding_rejections.push((p, r)));
    } else {
        idl::unroot(vm, r);
    }
}

/// HostPromiseRejectionTracker "handle" from the host: a promise in the outstanding rejected promises set
/// leaves it, and a `rejectionhandled` task is queued. (Called from inside js_core; the page state may be
/// borrowed by the binding that attached the handler, so the borrow is tried, never forced.)
pub(crate) fn rejection_handled(promise: Obj) {
    PAGE.with(|pg| {
        if let Ok(mut pg) = pg.try_borrow_mut() {
            if let Some(i) = pg.outstanding_rejections.iter().position(|(q, _)| *q == promise) {
                let (_, r) = pg.outstanding_rejections.remove(i);
                pg.tasks.push_back(Task::RejectionHandled(r));
            }
        }
    });
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
        range::reset();
        traversal::reset();
        controls::reset();
        forms::reset();
        events::reset_clock();
        clear_poison();
        crate::api::fetch::reset_budget();
        net::reset();
        crate::event_loop::reset();
        importmap::reset();
        crate::dom::mutation::reset();
        crate::dom::custom::reset(document.arena());
        page(|p| p.time_origin = crate::event_loop::now_ms());
        let mut vm = Vm::new(Box::new(host::AetherHost::default()));
        vm.track_rejections = true;
        idl::install_domexception(&mut vm);
        events::install(&mut vm);
        dom::install(&mut vm);
        // AETHERJS2: [Unscopable] members of ParentNode / ChildNode / Element (WebIDL §3.7.3).
        dom::install_unscopables(&mut vm);
        range::install(&mut vm);
        traversal::install(&mut vm);
        html::install(&mut vm);
        reflect::install(&mut vm);
        controls::install(&mut vm);
        cssom::install(&mut vm);
        window::install(&mut vm);
        net::install(&mut vm);
        window::install_scheduler(&mut vm);
        forms::install(&mut vm);
        // AETHERDEBT SR93: DataTransfer (its FileList feeds `input.files`) — after forms: File exists.
        controls::install_data_transfer(&mut vm);
        // After window::install: these replace the platform.rs MutationObserver/customElements stubs.
        shadow::install(&mut vm);
        mutation::install(&mut vm);
        custom::install(&mut vm);
        canvas::install(&mut vm);
        // AETHERSTORAGE (SR94): the inbox and serializer first, then the APIs that use them.
        broadcast::install(&mut vm);
        // AETHERWORKERS (SR123): MessagePort/MessageChannel, then Worker; the idle-callback task source.
        ports::install(&mut vm);
        worker::install(&mut vm);
        crate::event_loop::install(&mut vm);
        idb::install(&mut vm);
        cache_api::install(&mut vm);
        locks::install(&mut vm);
        sw::install(&mut vm);
        animations::install(&mut vm);
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
        net::note_url(url);
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
                Task::Rejection(r) => fire_rejection(&mut self.vm, r, false),
                Task::RejectionHandled(r) => fire_rejection(&mut self.vm, r, true),
            }
            checkpoint(&mut self.vm);
        }
        n
    }

    /// One event-loop turn: queued tasks, then the timers due on the page clock (each its own task),
    /// each followed by a microtask checkpoint.
    pub fn tick(&mut self) -> usize {
        let mut n = self.run_tasks();
        n += net::poll(&mut self.vm);
        n += broadcast::poll(&mut self.vm);
        n += animations::poll(&mut self.vm);
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

    /// Runs the queued network tasks (body chunks, socket messages, server-sent events, storage events
    /// from other documents). Returns how many ran.
    pub fn run_network_tasks(&mut self) -> usize {
        net::poll(&mut self.vm) + broadcast::poll(&mut self.vm)
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
