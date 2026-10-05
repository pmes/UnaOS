//! Script processing (HTML §4.12.1) driven by the HTML parser, `document.write` (§8.4.3), and the end
//! of parsing (§13.2.7).
//!
//! The document is parsed by html_core one token at a time ([`html_core::TreeBuilder::step_token`]).
//! When the tree builder pops a `<script>` at its end tag, the element is *prepared*:
//!
//! - inline classic scripts run at once (parser-blocking), with `document.currentScript` the element;
//! - external classic scripts without `defer`/`async` are parser-blocking: fetched (pre-fetched by the
//!   page load when possible, else synchronously through http_core / the file system) and run before
//!   the parser continues; a `load` (or `error`) event follows at the element;
//! - `defer` classic scripts and (non-`async`) module scripts, inline or external, join the "list of
//!   scripts that will execute when the document has finished parsing", in document order;
//! - `async` scripts join the "set of scripts that will execute as soon as possible" and run as tasks
//!   once parsing has finished (every resource is already available, so "as soon as possible" is the
//!   first turn after the parser yields).
//!
//! While a parser-inserted script runs, `document.write` inserts its markup at the insertion point
//! (the tokenizer position just after `</script>`) and spins the parser over exactly that input
//! before returning; a script written that way runs nested, and a written external script becomes the
//! pending parsing-blocking script, run when the writing script returns.
//!
//! Scripts inserted through DOM methods ("non-parser-inserted") are prepared when they become connected
//! (inline classic: at once; external or module: queued as tasks); scripts parsed by `innerHTML`,
//! `DOMParser` and friends are marked "already started" and never run.
//!
//! The arena: html_core's `TreeBuilder` owns the `Document` it builds. The page arena (what wrappers and
//! layout see) holds that same document whenever JavaScript can run: the loader swaps it into the arena
//! around every script and back to the tree builder for parsing.

use super::dom::{self, attr_value, with_doc};
use super::idl::throw_dom;
use super::{page, Engine, TargetKey};
use crate::dom::NodeRef;
use html_core::{Document, NodeId, Step, TokenizerOpts, Tokenizer, TreeBuilder};
use js_core::vm::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

/// How a prepared script runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScriptKind {
    Classic,
    Module,
}

/// A prepared script element's record.
#[derive(Clone, Debug)]
pub struct ScriptRec {
    pub kind: ScriptKind,
    pub url: Option<String>,
    /// The source (inline text, or the fetched resource); Err for a failed fetch.
    pub source: Option<Result<String, String>>,
    pub external: bool,
}

struct ParserCtx {
    tb: TreeBuilder,
    tz: Tokenizer,
    /// The document node the parser builds into is `Document::ROOT` of the main arena.
    /// A written external script waiting to block the parser.
    blocking: Option<usize>,
    /// The list of scripts that will execute when the document has finished parsing.
    deferred: Vec<usize>,
    /// The set of scripts that will execute as soon as possible.
    asap: Vec<usize>,
    /// The parser is inside an "end of script" (a script it prepared is running).
    nesting: u32,
}

thread_local! {
    static PARSER: RefCell<Option<Box<ParserCtx>>> = const { RefCell::new(None) };
    static SCRIPTS: RefCell<HashMap<usize, ScriptRec>> = RefCell::new(HashMap::new());
    /// `document.open()`ed documents with no parser: their written markup so far.
    static OPENED: RefCell<HashMap<usize, String>> = RefCell::new(HashMap::new());
    /// The executing script is external and was not inserted by the parser (`document.write` is ignored).
    static EXTERNAL_ASYNC_RUNNING: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn arena() -> crate::dom::Arena {
    dom::arena()
}

/// Moves the document out of the tree builder into the page arena (JavaScript may now run).
fn doc_to_arena(ctx: &mut ParserCtx) {
    std::mem::swap(&mut ctx.tb.doc, &mut *arena().borrow_mut());
}

/// Moves the document back to the tree builder (parsing continues).
fn doc_to_parser(ctx: &mut ParserCtx) {
    std::mem::swap(&mut ctx.tb.doc, &mut *arena().borrow_mut());
}

/// Runs `f` with the parser context taken out of its slot (so that a nested `document.write` can
/// take it again), putting it back after.
fn with_ctx<R>(f: impl FnOnce(&mut ParserCtx) -> R) -> Option<R> {
    let mut ctx = PARSER.with(|p| p.borrow_mut().take())?;
    let r = f(&mut ctx);
    PARSER.with(|p| *p.borrow_mut() = Some(ctx));
    Some(r)
}

fn take_ctx() -> Option<Box<ParserCtx>> {
    PARSER.with(|p| p.borrow_mut().take())
}

fn put_ctx(ctx: Box<ParserCtx>) {
    PARSER.with(|p| *p.borrow_mut() = Some(ctx));
}

// =================================================================================================
// Loading a document
// =================================================================================================

/// What the shell pre-fetched for a page: external script sources by absolute URL.
#[derive(Default, Clone)]
pub struct Prefetched {
    pub scripts: HashMap<String, String>,
}

/// Loads `html` as the document at `url`: parses it with script execution interleaved, runs "the
/// end" (deferred scripts, DOMContentLoaded, async scripts, load), and drains the boot event loop.
/// Returns the document and its engine.
pub fn load(url: &str, html: &str, pre: Prefetched, viewport: (f32, f32), external_css: Vec<String>) -> (NodeRef, Engine) {
    SCRIPTS.with(|s| s.borrow_mut().clear());
    OPENED.with(|s| s.borrow_mut().clear());
    PARSER.with(|p| *p.borrow_mut() = None);
    super::cssom::reset();
    let arena: crate::dom::Arena = Rc::new(RefCell::new(Document::new()));
    let doc = NodeRef::from_arena(arena.clone(), Document::ROOT);
    let mut engine = Engine::with_viewport(doc.clone(), viewport.0, viewport.1, external_css);
    engine.set_location(url);
    page(|p| p.script_sources = pre.scripts);

    // UNAOS_JSPRELUDE=<file>: a diagnostic script of our own, before the page's, in the same realm.
    if let Ok(path) = std::env::var("UNAOS_JSPRELUDE") {
        match std::fs::read_to_string(&path) {
            Ok(src) => {
                if let Err(e) = engine.execute(&src) {
                    eprintln!("[jsprelude] ERR: {e}");
                }
            }
            Err(e) => eprintln!("[jsprelude] cannot read {path}: {e}"),
        }
    }

    let tb = TreeBuilder::new(crate::dom::parse_opts());
    let tz = Tokenizer::from_str(html, TokenizerOpts { processing_instructions: false });
    let mut ctx = Box::new(ParserCtx { tb, tz, blocking: None, deferred: Vec::new(), asap: Vec::new(), nesting: 0 });
    // The tree builder starts with the (empty) document; the arena holds a placeholder while parsing.
    doc_to_parser(&mut ctx);
    put_ctx(ctx);
    parse_until_eof(&mut engine.vm);
    the_end(&mut engine);
    (doc, engine)
}

/// The main parse loop: tokens until EOF, preparing each script the tree builder hands over.
fn parse_until_eof(vm: &mut Vm) {
    loop {
        let Some(mut ctx) = take_ctx() else { return };
        let step = ctx.tb.step_token(&mut ctx.tz, None);
        let ready = ctx.tb.script_ready.take();
        if let Some(el) = ready {
            doc_to_arena(&mut ctx);
            put_ctx(ctx);
            prepare_parser_script(vm, el.0);
            // A written external script blocks the parser until it has run.
            loop {
                let blocking = with_ctx(|c| c.blocking.take()).flatten();
                match blocking {
                    Some(b) => {
                        run_script_now(vm, b);
                    }
                    None => break,
                }
            }
            let Some(mut ctx) = take_ctx() else { return };
            doc_to_parser(&mut ctx);
            put_ctx(ctx);
            continue;
        }
        if step == Step::Eof {
            ctx.tb.finish();
            doc_to_arena(&mut ctx);
            // Keep the context (with the deferred lists) for the end; the arena now owns the doc.
            put_ctx(ctx);
            return;
        }
        put_ctx(ctx);
    }
}

/// The script's type (HTML §4.12.1.1 "prepare the script element" steps 8–11): classic, module, or
/// not a script at all (a data block).
fn script_kind(el: usize) -> Option<ScriptKind> {
    let ty = attr_value(el, "type");
    let lang = attr_value(el, "language");
    let essence = match (ty, lang) {
        (Some(t), _) if !t.trim().is_empty() => t.trim().to_ascii_lowercase(),
        (None, Some(l)) if !l.is_empty() => format!("text/{}", l.to_ascii_lowercase()),
        _ => return Some(ScriptKind::Classic),
    };
    let essence = essence.split(';').next().unwrap_or("").trim().to_string();
    const JS: &[&str] = &[
        "application/ecmascript", "application/javascript", "application/x-ecmascript", "application/x-javascript",
        "text/ecmascript", "text/javascript", "text/javascript1.0", "text/javascript1.1", "text/javascript1.2",
        "text/javascript1.3", "text/javascript1.4", "text/javascript1.5", "text/jscript", "text/livescript",
        "text/x-ecmascript", "text/x-javascript",
    ];
    if JS.contains(&essence.as_str()) {
        return Some(ScriptKind::Classic);
    }
    if essence == "module" {
        return Some(ScriptKind::Module);
    }
    None
}

fn inline_text(el: usize) -> String {
    with_doc(|d| {
        d.children(NodeId(el))
            .filter_map(|c| match d.data(c) {
                html_core::NodeData::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    })
}

/// Fetches an external script's source: the page's pre-fetched set first, then the network/disk.
fn fetch_source(url: &str) -> Result<String, String> {
    if let Some(s) = page(|p| p.script_sources.get(url).cloned()) {
        return Ok(s);
    }
    let r = super::host::fetch_script_text(url);
    if let Err(e) = &r {
        crate::ledger::record_js(&format!("script-fetch-failed:{}", super::clip(url, 48)));
        if std::env::var("UNAOS_JSDEBUG").is_ok() {
            eprintln!("[jsdebug] fetch {url} failed: {e}");
        }
    }
    r
}

/// Builds the script record of `el` (steps of "prepare the script element" that decide type, source
/// and URL). None when it is not a script to run.
fn make_record(el: usize) -> Option<ScriptRec> {
    let kind = script_kind(el)?;
    if kind == ScriptKind::Classic && attr_value(el, "nomodule").is_some() {
        return None;
    }
    match attr_value(el, "src") {
        Some(src) => {
            let url = dom::resolve_url(dom::node_document(el), &src);
            Some(ScriptRec { kind, url, source: None, external: true })
        }
        None => Some(ScriptRec { kind, url: None, source: Some(Ok(inline_text(el))), external: false }),
    }
}

/// "prepare the script element" for a parser-inserted script at its end tag.
fn prepare_parser_script(vm: &mut Vm, el: usize) {
    page(|p| {
        p.parser_inserted.insert(el);
    });
    if page(|p| p.started.contains(&el)) {
        return;
    }
    // A script with no src and empty text is not started (step 4); being parser-inserted, it never will be.
    if attr_value(el, "src").is_none() && inline_text(el).is_empty() {
        return;
    }
    page(|p| p.started.insert(el));
    let Some(rec) = make_record(el) else { return };
    if !dom::is_connected(el) {
        return;
    }
    let is_async = attr_value(el, "async").is_some();
    let is_defer = attr_value(el, "defer").is_some();
    let kind = rec.kind;
    let external = rec.external;
    SCRIPTS.with(|s| s.borrow_mut().insert(el, rec));
    let writing = with_ctx(|c| c.nesting).unwrap_or(0) > 0;
    match (kind, external) {
        (ScriptKind::Classic, false) => run_script_now(vm, el),
        (ScriptKind::Classic, true) if is_async => {
            with_ctx(|c| c.asap.push(el));
        }
        (ScriptKind::Classic, true) if is_defer => {
            with_ctx(|c| c.deferred.push(el));
        }
        (ScriptKind::Classic, true) => {
            if writing {
                // A script document.write wrote: it becomes the pending parsing-blocking script.
                with_ctx(|c| c.blocking = Some(el));
            } else {
                run_script_now(vm, el);
            }
        }
        (ScriptKind::Module, _) if is_async => {
            with_ctx(|c| c.asap.push(el));
        }
        (ScriptKind::Module, _) => {
            with_ctx(|c| c.deferred.push(el));
        }
    }
}

/// Fetches (when needed) and executes a prepared script, firing `load`/`error` for external ones.
fn run_script_now(vm: &mut Vm, el: usize) {
    let Some(mut rec) = SCRIPTS.with(|s| s.borrow().get(&el).cloned()) else { return };
    if rec.source.is_none() {
        rec.source = Some(match &rec.url {
            Some(u) => fetch_source(u),
            None => Err("unresolvable script URL".into()),
        });
        SCRIPTS.with(|s| s.borrow_mut().insert(el, rec.clone()));
    }
    let src = match rec.source.clone().unwrap() {
        Ok(s) => s,
        Err(_) => {
            super::events::fire_simple(vm, TargetKey::Node(el), "error", false, false);
            return;
        }
    };
    let debug = std::env::var("UNAOS_JSDEBUG").is_ok();
    if let Ok(dir) = std::env::var("UNAOS_JSDUMP") {
        let _ = std::fs::create_dir_all(&dir);
        let n = page(|p| p.started.len());
        let _ = std::fs::write(format!("{dir}/script-{n:03}.js"), &src);
    }
    let parser_inserted = page(|p| p.parser_inserted.contains(&el));
    if rec.external && !parser_inserted {
        EXTERNAL_ASYNC_RUNNING.with(|c| c.set(c.get() + 1));
    }
    let r = match rec.kind {
        ScriptKind::Classic => {
            let prev = page(|p| std::mem::replace(&mut p.current_script, Some(el)));
            let nest = with_ctx(|c| {
                c.nesting += 1;
            })
            .is_some();
            let r = super::run_classic_no_checkpoint(vm, &src, "script");
            if nest {
                with_ctx(|c| c.nesting -= 1);
            }
            page(|p| p.current_script = prev);
            if super::stack_empty() {
                super::checkpoint(vm);
            }
            r.map(|_| ())
        }
        ScriptKind::Module => run_module(vm, el, &rec, &src),
    };
    if rec.external && !parser_inserted {
        EXTERNAL_ASYNC_RUNNING.with(|c| c.set(c.get().saturating_sub(1)));
    }
    if debug {
        let head: String = src.chars().take(60).filter(|c| !c.is_control()).collect();
        match &r {
            Ok(()) => eprintln!("[jsdebug] script ok | head: {head}"),
            Err(_) => eprintln!("[jsdebug] script ERR | head: {head}"),
        }
    }
    if rec.external {
        super::events::fire_simple(vm, TargetKey::Node(el), "load", false, false);
    }
}

fn module_rejected(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let e = vm.arg(ctx, 0);
    super::events::report_exception(vm, &e);
    Ok(Value::Undefined)
}

/// Runs a module script (fetch-the-descendants through `Host::load_module`, link, evaluate). An
/// evaluation error — synchronous or from top-level await — is reported like a classic script's.
fn run_module(vm: &mut Vm, el: usize, rec: &ScriptRec, src: &str) -> Result<(), Value> {
    let name = match &rec.url {
        Some(u) => u.clone(),
        None => {
            let n = page(|p| {
                p.inline_module_seq += 1;
                p.inline_module_seq
            });
            let base = dom::base_url(dom::node_document(el));
            format!("{}#inline-module-{n}", base.split('#').next().unwrap_or(&base))
        }
    };
    let r = super::enter(vm, "module", |vm| -> Result<Option<Obj>, Value> {
        let p = vm.run_module(&name, src)?;
        Ok(Some(p))
    });
    let out = match r {
        Some(Ok(Some(p))) => {
            // Handle the evaluation promise: a rejection is a script error, not an unhandled rejection.
            let state = match &vm.heap.get(p).kind {
                Kind::Promise(d) => Some((d.state, d.result.clone())),
                _ => None,
            };
            match state {
                Some((PromiseState::Rejected, reason)) => {
                    if let Kind::Promise(d) = &mut vm.heap.get_mut(p).kind {
                        d.handled = true;
                    }
                    vm.rejected_unhandled.retain(|&q| q != p);
                    super::events::report_exception(vm, &reason);
                    Err(reason)
                }
                Some((PromiseState::Pending, _)) => {
                    let fp = vm.intr().function_proto;
                    let rej = vm.make_native_with("", 1, module_rejected, false, Some(fp), Vec::new());
                    js_core::builtins::promise::perform_then(vm, p, Value::Undefined, Value::Object(rej), None);
                    Ok(())
                }
                _ => Ok(()),
            }
        }
        Some(Ok(None)) => Ok(()),
        Some(Err(e)) => {
            super::events::report_exception(vm, &e);
            Err(e)
        }
        None => Err(Value::str("js engine poisoned")),
    };
    if super::stack_empty() {
        super::checkpoint(vm);
    }
    out
}

/// HTML §13.2.7 "the end", then the boot event-loop drain.
fn the_end(engine: &mut Engine) {
    let vm = &mut engine.vm;
    let Some(ctx) = take_ctx() else { return };
    let deferred = ctx.deferred.clone();
    let asap = ctx.asap.clone();
    drop(ctx);
    let doc = dom::main_doc();
    page(|p| p.ready_state = "interactive");
    super::events::fire_simple(vm, TargetKey::Node(doc), "readystatechange", false, false);
    super::checkpoint(vm);
    for el in deferred {
        run_script_now(vm, el);
        super::checkpoint(vm);
    }
    super::events::fire_simple(vm, TargetKey::Node(doc), "DOMContentLoaded", true, false);
    super::checkpoint(vm);
    for el in asap {
        run_script_now(vm, el);
        super::checkpoint(vm);
    }
    // Tasks the scripts queued (dynamically inserted scripts, messages) and the zero-delay timers get
    // their turns before the load event, bounded.
    crate::event_loop::boot_drain(engine);
    let vm = &mut engine.vm;
    page(|p| p.ready_state = "complete");
    super::events::fire_simple(vm, TargetKey::Node(doc), "readystatechange", false, false);
    super::checkpoint(vm);
    super::events::fire_simple(vm, TargetKey::Window, "load", false, false);
    super::checkpoint(vm);
    crate::event_loop::boot_drain(engine);
    engine.drain_raf();
    crate::event_loop::boot_drain(engine);
}

// =================================================================================================
// Dynamically inserted scripts
// =================================================================================================

/// Post-insertion steps for a node that was inserted (DOM "insert" → HTML script element steps): every
/// connected, not-yet-started script among its inclusive descendants is prepared.
pub fn node_inserted(vm: &mut Vm, n: usize) {
    if !dom::is_connected(n) {
        return;
    }
    let scripts: Vec<usize> = with_doc(|d| {
        std::iter::once(NodeId(n))
            .chain(d.descendants(NodeId(n)))
            .filter(|i| d.element(*i).is_some_and(|e| e.is_html("script")))
            .map(|i| i.0)
            .collect()
    });
    for s in scripts {
        prepare_dynamic_script(vm, s);
    }
}

/// "children changed steps" of a script element: a connected, unstarted, script-created script whose
/// text arrives now is prepared.
pub fn children_changed(vm: &mut Vm, parent: usize) {
    let is_script = with_doc(|d| d.element(NodeId(parent)).is_some_and(|e| e.is_html("script")));
    if is_script && dom::is_connected(parent) && !page(|p| p.parser_inserted.contains(&parent)) {
        prepare_dynamic_script(vm, parent);
    }
}

fn prepare_dynamic_script(vm: &mut Vm, el: usize) {
    if page(|p| p.started.contains(&el) || p.parser_inserted.contains(&el)) {
        return;
    }
    // A parser-created script whose end tag has not been seen yet is the parser's to prepare.
    if PARSER.with(|p| p.borrow().is_some()) && page(|p| p.current_script.is_none()) && !super::stack_empty() {
        // (only scripts can insert nodes while parsing; this branch is for completeness)
    }
    if attr_value(el, "src").is_none() && inline_text(el).is_empty() {
        return;
    }
    page(|p| p.started.insert(el));
    let Some(rec) = make_record(el) else { return };
    let kind = rec.kind;
    let external = rec.external;
    SCRIPTS.with(|s| s.borrow_mut().insert(el, rec));
    if kind == ScriptKind::Classic && !external {
        run_script_now(vm, el);
    } else {
        page(|p| p.tasks.push_back(super::Task::Script(el)));
    }
}

/// Runs a queued script task (an async or dynamically inserted script whose source is available).
pub fn execute_ready_script(vm: &mut Vm, el: usize) {
    run_script_now(vm, el);
}

// =================================================================================================
// document.write / open / close
// =================================================================================================

/// `document.write(text)` (HTML §8.4.3).
pub fn document_write(vm: &mut Vm, doc: usize, text: &str) -> JsResult<()> {
    let main = dom::main_doc();
    if doc == main && PARSER.with(|p| p.borrow().is_some()) {
        // An active parser with an insertion point: a script it is running wrote.
        let nesting = with_ctx(|c| c.nesting).unwrap_or(0);
        if nesting > 0 {
            write_into_parser(vm, text);
            return Ok(());
        }
    }
    if EXTERNAL_ASYNC_RUNNING.with(|c| c.get()) > 0 && doc == main && !OPENED.with(|o| o.borrow().contains_key(&doc)) {
        super::host::console_out(1, "Failed to execute 'write' on 'Document': It isn't possible to write into a document from an asynchronously-loaded external script unless it is explicitly opened.");
        return Ok(());
    }
    // No insertion point: document.open() first (once), then the markup accumulates.
    if !OPENED.with(|o| o.borrow().contains_key(&doc)) {
        document_open(vm, doc)?;
    }
    OPENED.with(|o| o.borrow_mut().entry(doc).or_default().push_str(text));
    reparse_opened(vm, doc);
    Ok(())
}

/// Inserts `text` at the parser's insertion point and spins the parser over it.
fn write_into_parser(vm: &mut Vm, text: &str) {
    let Some(mut ctx) = take_ctx() else { return };
    let limit = ctx.tz.insert_at_cursor(text);
    doc_to_parser(&mut ctx);
    loop {
        if ctx.blocking.is_some() {
            break;
        }
        let step = ctx.tb.step_token(&mut ctx.tz, Some(limit));
        if let Some(el) = ctx.tb.script_ready.take() {
            doc_to_arena(&mut ctx);
            put_ctx(ctx);
            prepare_parser_script(vm, el.0);
            let Some(c) = take_ctx() else { return };
            ctx = c;
            doc_to_parser(&mut ctx);
            continue;
        }
        if step != Step::Token {
            break;
        }
        if ctx.tz.position() >= limit {
            // Drain tokens already queued for the written text, then stop at the insertion point.
            continue;
        }
    }
    doc_to_arena(&mut ctx);
    put_ctx(ctx);
}

/// `document.open()`: an opened document's content is replaced by what is written to it.
pub fn document_open(vm: &mut Vm, doc: usize) -> JsResult<()> {
    let main = dom::main_doc();
    if doc == main && PARSER.with(|p| p.borrow().is_some()) {
        // Opening a document that is being parsed is ignored (it has an active parser).
        return Ok(());
    }
    if !dom::is_html_doc(doc) {
        return throw_dom(vm, "InvalidStateError", "Only HTML documents can be opened.");
    }
    OPENED.with(|o| o.borrow_mut().insert(doc, String::new()));
    let kids = dom::children_of(doc);
    dom::with_doc_mut(|d| {
        for k in kids {
            d.detach(NodeId(k));
        }
    });
    super::touch();
    crate::ledger::record_dom("document.open-replaces-document");
    Ok(())
}

/// `document.close()`.
pub fn document_close(vm: &mut Vm, doc: usize) -> JsResult<()> {
    if OPENED.with(|o| o.borrow_mut().remove(&doc)).is_some() {
        let _ = vm;
    }
    Ok(())
}

/// Re-parses an opened document's accumulated markup into it (its scripts marked already started).
fn reparse_opened(vm: &mut Vm, doc: usize) {
    let markup = OPENED.with(|o| o.borrow().get(&doc).cloned()).unwrap_or_default();
    let parsed = html_core::parse_document(&markup, crate::dom::parse_opts());
    let kids = dom::children_of(doc);
    dom::with_doc_mut(|d| {
        for k in kids {
            d.detach(NodeId(k));
        }
    });
    let top: Vec<NodeId> = parsed.children(Document::ROOT).collect();
    for t in top {
        let id = dom::with_doc_mut(|d| crate::dom::import_tree(d, &parsed, t, false).0);
        dom::with_doc_mut(|d| d.append(NodeId(doc), NodeId(id)));
        let ids: Vec<usize> = with_doc(|d| std::iter::once(NodeId(id)).chain(d.descendants(NodeId(id))).map(|i| i.0).collect());
        for i in ids {
            dom::set_node_doc(i, doc);
            if with_doc(|d| d.element(NodeId(i)).is_some_and(|e| e.is_html("script"))) {
                page(|p| p.started.insert(i));
            }
        }
    }
    super::touch();
    let _ = vm;
}
