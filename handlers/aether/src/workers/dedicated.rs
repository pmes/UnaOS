//! Dedicated workers (AETHERWORKERS, LEDGER SR123; HTML §10.2 DedicatedWorkerGlobalScope, §10.2.4 "run a
//! worker").
//!
//! CHARTER: Aether — engine (the worker half of `js::worker`; the page-facing `Worker` object is there).
//!
//! # Threads and the page's task source
//! Each `new Worker()` is one thread with its own `js_core` realm — no DOM — built the way the service-worker
//! realm is (`workers::realm`): the host globals (console, the timer family, `queueMicrotask`), the natives it
//! shares with that realm (`realm::install_shared`: URL resolution, `fetch` as a client of the worker's origin,
//! UTF-8, the Cache API over the page's storage key, the ledger), the structured serializer
//! (`js::broadcast::SERIALIZE`), the DOM-less platform prelude (`api::platform::init_worker`: URL,
//! TextEncoder/Decoder, AbortController, crypto), `Headers`/`Request`/`Response`/`fetch` (`realm::FETCH`), the
//! Cache API shapes, MessagePort/MessageChannel (`js::ports`), and this module's [`EVENTS`] and [`SCOPE`]
//! preludes (Event/EventTarget/MessageEvent/ErrorEvent, the global as a DedicatedWorkerGlobalScope, `self`,
//! `location`, `navigator`, `name`, `postMessage`, `close`, `importScripts`, `onmessage`/`onmessageerror`/
//! `onerror`, `XMLHttpRequest`, `Blob`/`File`, `performance`).
//!
//! The page fetched the script (through its loader, under its CSP and origin — `js::worker`) before the thread
//! starts; the thread evaluates it (classic: ScriptEvaluation; module: `run_module`, whose static imports load
//! through [`WorkerHost::load_module`]), then runs [`Cmd`]s as tasks: a `Message` from the page fires `message`
//! (or `messageerror`) at the global, a `Port` delivers to one of the realm's MessagePorts. Between commands the
//! thread fires its own timers on REAL time (js_core's built-in timers carry `due = loop_now + delay`; every
//! task first sets `loop_now` to the worker's elapsed milliseconds, and [`fire_due_timers`] runs what is due,
//! one generation per pass, the way `event_loop::fire_due_timers` does for the page). Each task runs under
//! `js::TASK_BUDGET` and ends with a microtask checkpoint.
//!
//! The worker posts to the page through the `js::broadcast` inbox (`worker-message`, `worker-error`), so every
//! worker message is a task on the page's event loop, run by `broadcast::poll` from `Engine::tick` and the
//! event-loop drivers. While a worker has commands outstanding or a timer due within [`SOON_MS`], [`busy`]
//! answers true and `event_loop::settle` waits on real time for it, as it does for network operations.
//!
//! # The worker's inbox and nested workers (AETHERWORKERS2, SR125)
//! Each worker thread takes an inbox serial of its own from `js::broadcast::enter_worker`, in the documents'
//! serial space: `js::broadcast::post(serial, kind, payload)` becomes a [`Cmd::Inbox`] task that runs the
//! realm's `__inbox` dispatcher. Its nested workers post to it (`worker-message`, `worker-error` — a child's
//! [`Spawn::owner`] is its parent's serial), and the realm runs `js::worker`'s own binding, whose spawn path
//! reads [`creator`]: the worker's URL as the base and origin, its captured request context ([`rebase`]d
//! from the creator's: the document's CSP, referrer policy and loader policy, the worker as client), the
//! owner document's storage key. A worker that ends or is terminated terminates its children.
//!
//! # Errors and termination
//! An uncaught exception (in the script, a task, a timer, a listener, a microtask) is reported: an ErrorEvent
//! at the global (cancelable; `onerror(message, filename, lineno, colno, error)` returning true cancels), and if
//! not canceled a `worker-error` to the page, which fires an ErrorEvent at the Worker object. `lineno`/`colno`
//! are exact for a SyntaxError in the worker's own script (re-parsed for its position) and 0 otherwise:
//! js_core's error traces carry no source positions.
//!
//! `worker.terminate()` (and the document going away) sets the worker's kill flag, drops it from the registry
//! so nothing more is delivered, and wakes the thread with `Stop`. The kill flag is also the realm's
//! `Vm::interrupt` (AETHERWORKERS2, SR125): js_core checks it beside the instruction budget in `run_inner` and
//! `Vm::tick`, so a running script — a pure compute loop that calls nothing included — stops at its next
//! instruction, `vm.terminated` set, exactly as an exhausted budget stops it. Between tasks the loop checks
//! the flag; the natives' `checkpoint_kill` stays as the belt to that brace.

use js_core::vm::*;
use serde_json::json;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

/// A worker whose next timer is due within this many milliseconds counts as [`busy`].
pub const SOON_MS: f64 = 100.0;

/// Most timers one pass of [`fire_due_timers`] fires.
pub const MAX_FIRES_PER_PASS: usize = 256;

/// Most module scripts one worker loads.
pub const MODULE_CAP: usize = 256;

/// What the worker thread is asked to do.
pub enum Cmd {
    /// `worker.postMessage`: a serialized message and its transferred port ids (comma-separated).
    Message { data: String, ports: String },
    /// A message for one of the realm's MessagePorts (`js::ports`).
    Port { port: u64, data: String, ports: String },
    /// A message for the realm's inbox (AETHERWORKERS2, SR125): its nested workers' `worker-message` /
    /// `worker-error`, the storage family's grants and notices — whatever `js::broadcast::post` sends to the
    /// worker's serial. Runs `__inbox(kind, payload)` as a task.
    Inbox { kind: String, payload: String },
    Stop,
}

/// The worker's script type (`WorkerOptions.type`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Classic,
    Module,
}

/// Everything a worker thread starts from.
pub struct Spawn {
    pub id: u64,
    /// The owning document (its `js::broadcast` inbox serial): the worker dies with it.
    pub doc: u64,
    /// The inbox serial of the realm that created the worker — the document's, or the parent worker's for a
    /// nested worker (AETHERWORKERS2, SR125): the worker's `worker-message` / `worker-error` go there.
    pub owner: u64,
    /// The parent worker of a nested worker (None: a document's worker); the child dies with it.
    pub parent: Option<u64>,
    /// The script's URL (the worker's location; its origin is the worker's origin).
    pub url: String,
    pub kind: Kind,
    pub name: String,
    /// The script, fetched by the page.
    pub source: String,
    /// The page's storage key ("" for an opaque origin: no Cache API).
    pub storage_key: String,
    /// The page's navigator snapshot (JSON) the WorkerNavigator mirrors.
    pub navigator: String,
    /// The owner document's secure-context flag (Web Locks).
    pub secure: bool,
    /// The creating realm's request context, destination `worker` (`js::host::worker_request_context`, or the
    /// parent worker's): the worker re-bases it on its own URL for `importScripts`, module imports and nested
    /// workers — the page-thread facts (origin, CSP, referrer policy) a worker thread cannot read.
    pub ctx: crate::security::RequestContext,
}

/// What `new Worker()` takes from the realm that calls it (HTML §10.2.6 "outside settings"): the document's
/// facts on the page thread (`js::worker` builds them), the owning worker's captured ones on a worker thread.
pub struct Creator {
    /// The base URL the script URL resolves against (the document's, or the worker's location).
    pub base: String,
    pub doc: u64,
    /// The creating realm's inbox serial.
    pub owner: u64,
    pub parent: Option<u64>,
    pub storage_key: String,
    pub secure: bool,
    pub ctx: crate::security::RequestContext,
}

/// On a dedicated worker's thread: the worker's facts as the creator of a nested worker; None elsewhere.
pub fn creator() -> Option<Creator> {
    with_w(|w| Creator {
        base: w.url.clone(),
        doc: w.doc,
        owner: w.serial,
        parent: Some(w.id),
        storage_key: w.storage_key.clone(),
        secure: w.secure,
        ctx: w.ctx.clone(),
    })
}

/// A request context re-based on the worker as its client: the worker's URL is the client and the referrer,
/// its origin the request's origin; the document's policies stay (a worker's own CSP header is not read).
fn rebase(mut ctx: crate::security::RequestContext, url: &str) -> crate::security::RequestContext {
    if ctx.has_page {
        ctx.client_url = url.to_string();
        ctx.client_origin = crate::security::origin::Origin::of(url);
        ctx.referrer = Some(url.to_string());
    }
    ctx
}

struct Shared {
    /// Set by `terminate` (and at thread exit); the realm's `Vm::interrupt`.
    kill: Arc<AtomicBool>,
    /// Commands sent and not yet run (plus the script evaluation, until it ends).
    pending: AtomicUsize,
    /// A timer is due within [`SOON_MS`].
    timer_soon: AtomicBool,
}

struct Handle {
    doc: u64,
    parent: Option<u64>,
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
}

fn workers() -> MutexGuard<'static, HashMap<u64, Handle>> {
    static W: OnceLock<Mutex<HashMap<u64, Handle>>> = OnceLock::new();
    W.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

/// A fresh worker id (the page's `Worker` object is keyed by it before the thread exists).
pub fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed) + 1
}

/// Starts the worker thread. The script is evaluated on it; its errors arrive at the page as `worker-error`.
pub fn spawn(sp: Spawn) -> Result<(), String> {
    let (tx, rx) = channel::<Cmd>();
    let shared = Arc::new(Shared { kill: Arc::new(AtomicBool::new(false)), pending: AtomicUsize::new(1), timer_soon: AtomicBool::new(false) });
    let id = sp.id;
    workers().insert(id, Handle { doc: sp.doc, parent: sp.parent, tx, shared: shared.clone() });
    let r = std::thread::Builder::new().name("aether-dedicated-worker".into()).stack_size(8 << 20).spawn(move || worker_main(sp, rx, shared));
    if let Err(e) = r {
        workers().remove(&id);
        return Err(e.to_string());
    }
    Ok(())
}

fn send(id: u64, cmd: Cmd) -> bool {
    let w = workers();
    let Some(h) = w.get(&id) else { return false };
    if h.shared.kill.load(Ordering::Relaxed) {
        return false;
    }
    h.shared.pending.fetch_add(1, Ordering::SeqCst);
    if h.tx.send(cmd).is_err() {
        h.shared.pending.fetch_sub(1, Ordering::SeqCst);
        return false;
    }
    true
}

/// `worker.postMessage` from the page: one `message` task on the worker; false when it is gone.
pub fn post(id: u64, data: String, ports: String) -> bool {
    send(id, Cmd::Message { data, ports })
}

/// A MessagePort message for a port worker `id` owns (`js::ports`).
pub fn deliver_port(id: u64, port: u64, data: &str, ports: &str) -> bool {
    send(id, Cmd::Port { port, data: data.to_string(), ports: ports.to_string() })
}

/// `worker.terminate()`: nothing more is delivered either way; the running script is interrupted at its next
/// instruction (`Vm::interrupt`); the worker's nested workers terminate with it.
pub fn terminate(id: u64) {
    let h = workers().remove(&id);
    if let Some(h) = h {
        h.shared.kill.store(true, Ordering::SeqCst);
        let _ = h.tx.send(Cmd::Stop);
    }
    crate::js::ports::owner_gone(crate::js::ports::Owner::Worker(id));
    terminate_children(id);
}

/// HTML §10.2.3: a worker whose owner set empties is terminated — the nested workers of `parent`.
fn terminate_children(parent: u64) {
    let ids: Vec<u64> = workers().iter().filter(|(_, h)| h.parent == Some(parent)).map(|(id, _)| *id).collect();
    for id in ids {
        terminate(id);
    }
}

/// The document went away: its workers terminate.
pub fn doc_gone(doc: u64) {
    let ids: Vec<u64> = workers().iter().filter(|(_, h)| h.doc == doc).map(|(id, _)| *id).collect();
    for id in ids {
        terminate(id);
    }
}

/// Whether one of `doc`'s workers has work outstanding (commands not yet run, or a timer due within
/// [`SOON_MS`]): the virtual-time drivers wait on real time for it.
pub fn busy(doc: u64) -> bool {
    workers().values().any(|h| h.doc == doc && !h.shared.kill.load(Ordering::Relaxed) && (h.shared.pending.load(Ordering::SeqCst) > 0 || h.shared.timer_soon.load(Ordering::Relaxed)))
}

/// How many of `doc`'s workers are alive (registered and not terminated).
pub fn live(doc: u64) -> usize {
    workers().values().filter(|h| h.doc == doc && !h.shared.kill.load(Ordering::Relaxed)).count()
}

fn threads_map() -> MutexGuard<'static, HashMap<u64, usize>> {
    static T: OnceLock<Mutex<HashMap<u64, usize>>> = OnceLock::new();
    T.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|e| e.into_inner())
}

/// How many worker THREADS of `doc` are still running (a terminated worker counts until its thread reaches
/// a checkpoint and exits).
pub fn threads(doc: u64) -> usize {
    threads_map().get(&doc).copied().unwrap_or(0)
}

// ------------------------------------------------------------------------------------------- the worker thread

struct WState {
    id: u64,
    doc: u64,
    /// This worker's own inbox serial (`js::broadcast::enter_worker`).
    serial: u64,
    /// The creating realm's inbox serial.
    owner: u64,
    storage_key: String,
    secure: bool,
    /// The request context re-based on this worker ([`rebase`]).
    ctx: crate::security::RequestContext,
    url: String,
    kind: Kind,
    shared: Arc<Shared>,
    closing: bool,
    start: Instant,
}

thread_local! {
    static W: RefCell<Option<WState>> = const { RefCell::new(None) };
    /// An error report is being dispatched (an `error` listener that throws is logged, not re-reported).
    static REPORTING: Cell<bool> = const { Cell::new(false) };
}

fn with_w<R>(f: impl FnOnce(&mut WState) -> R) -> Option<R> {
    W.with(|w| w.borrow_mut().as_mut().map(f))
}

fn elapsed_ms() -> f64 {
    with_w(|w| w.start.elapsed().as_secs_f64() * 1000.0).unwrap_or(0.0)
}

fn killed() -> bool {
    with_w(|w| w.shared.kill.load(Ordering::Relaxed)).unwrap_or(true)
}

/// A worker native's checkpoint: once terminated, the running script stops at its next instruction.
fn checkpoint_kill(vm: &mut Vm) -> JsResult<()> {
    if killed() {
        vm.budget = Some(0);
        vm.terminated = true;
        return Err(Value::str("worker terminated"));
    }
    Ok(())
}

fn s(v: &str) -> Value {
    Value::String(js_core::string::JsStr::from_str(v))
}

fn str_arg(vm: &mut Vm, c: &CallCtx, i: usize) -> JsResult<String> {
    let v = vm.arg(c, i);
    if v.is_nullish() {
        return Ok(String::new());
    }
    Ok(vm.to_string(&v)?.to_rust())
}

struct WorkerHost {
    url: String,
    modules: usize,
    /// The worker's request context: module imports go out as `cors` requests of destination `worker`.
    ctx: crate::security::RequestContext,
}

impl Host for WorkerHost {
    fn console(&mut self, level: u8, msg: &str) {
        let tag = ["log", "warn", "error", "debug", "info"].get(level as usize).copied().unwrap_or("log");
        eprintln!("[aether-worker] {tag}: {msg}");
    }
    fn now_ms(&mut self) -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| (d.as_secs_f64() * 1000.0).floor()).unwrap_or(0.0)
    }
    /// A module worker's static and dynamic imports: resolved against the importing module (bare specifiers
    /// refuse — a worker has no import map), fetched under the worker's request context (destination
    /// `worker`, mode `cors`, a JavaScript MIME type required: `js::host::fetch_worker_script`).
    fn load_module(&mut self, referrer: Option<&str>, specifier: &str) -> Result<(String, String), String> {
        let base = referrer.unwrap_or(&self.url).to_string();
        let bare = !(specifier.starts_with('/') || specifier.starts_with("./") || specifier.starts_with("../") || specifier.contains(':'));
        if bare {
            return Err(format!("Failed to resolve module specifier \"{specifier}\": a worker has no import map, so relative references must start with \"/\", \"./\" or \"../\"."));
        }
        let resolved = html_core::url::Url::parse(&base).and_then(|b| b.join(specifier)).map_err(|_| format!("Invalid module URL {specifier}"))?.to_string();
        self.modules += 1;
        if self.modules > MODULE_CAP {
            return Err(format!("module fetch cap reached loading {resolved}"));
        }
        let mut ctx = self.ctx.clone();
        ctx.mode = "cors".into();
        let (src, _) = crate::js::host::fetch_worker_script(&resolved, &ctx).map_err(|e| crate::js::host::refusal_message(&e))?;
        Ok((resolved, src))
    }
}

/// `__w_post(data, ports)`: `postMessage` from the worker — a `worker-message` task on the page.
fn post_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    checkpoint_kill(vm)?;
    let data = str_arg(vm, c, 0)?;
    let ports = str_arg(vm, c, 1)?;
    let (id, owner) = with_w(|w| (w.id, w.owner)).unwrap_or((0, 0));
    crate::js::broadcast::post(owner, "worker-message", json!({ "id": id, "data": data, "ports": ports }).to_string());
    Ok(Value::Undefined)
}

/// `__w_close()`: `self.close()` — the thread ends after the running task; queued tasks are discarded.
fn close_native(vm: &mut Vm, _c: &CallCtx) -> JsResult<Value> {
    checkpoint_kill(vm)?;
    with_w(|w| w.closing = true);
    Ok(Value::Undefined)
}

/// `__w_import(url)`: one script of `importScripts` — fetched synchronously under the worker's request context
/// (destination `script`, mode `no-cors`, a JavaScript MIME type required — HTML §10.3.4.1) and run in the
/// realm. A failed fetch answers its message (the prelude throws the NetworkError DOMException in the realm's
/// own class).
fn import_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    checkpoint_kill(vm)?;
    let url = str_arg(vm, c, 0)?;
    let mut ctx = with_w(|w| w.ctx.clone()).unwrap_or_else(crate::security::RequestContext::none);
    ctx.destination = "script".into();
    ctx.mode = "no-cors".into();
    ctx.credentials = "include".into();
    let src = match crate::js::host::fetch_worker_script(&url, &ctx) {
        Ok((t, _)) => t,
        Err(e) => {
            let e = crate::js::host::refusal_message(&e);
            return Ok(s(&format!("Failed to execute 'importScripts' on 'WorkerGlobalScope': The script at '{url}' failed to load: {e}")));
        }
    };
    vm.run_script_str(&src)?;
    checkpoint_kill(vm)?;
    Ok(Value::Undefined)
}

/// `__w_now()`: milliseconds since the worker started (`performance.now()`).
fn now_native(vm: &mut Vm, _c: &CallCtx) -> JsResult<Value> {
    checkpoint_kill(vm)?;
    Ok(Value::Number(elapsed_ms()))
}

/// `__w_forward_error(message, filename, lineno, colno)`: a nested worker's error that neither its Worker
/// object nor this worker's global canceled goes on to this worker's owner (HTML §10.2.5, one level up).
fn forward_error_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    checkpoint_kill(vm)?;
    let message = str_arg(vm, c, 0)?;
    let filename = str_arg(vm, c, 1)?;
    let lineno = vm.arg(c, 2);
    let lineno = vm.to_number(&lineno)?;
    let colno = vm.arg(c, 3);
    let colno = vm.to_number(&colno)?;
    let (id, owner) = with_w(|w| (w.id, w.owner)).unwrap_or((0, 0));
    crate::js::broadcast::post(owner, "worker-error", json!({ "id": id, "message": message, "filename": filename, "lineno": lineno, "colno": colno }).to_string());
    Ok(Value::Undefined)
}

/// `__w_uncaught(error)`: an exception a listener threw (EventTarget dispatch reports it, HTML §8.1.4.6).
fn uncaught_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    checkpoint_kill(vm)?;
    let e = vm.arg(c, 0);
    report(vm, &e, None);
    Ok(Value::Undefined)
}

fn call_global(vm: &mut Vm, name: &str, args: &[Value]) -> JsResult<Value> {
    let g = vm.realm().global;
    let f = vm.get(g, &PropertyKey::from_str(name))?;
    if !vm.is_callable(&f) {
        return Ok(Value::Undefined);
    }
    vm.call(&f, &Value::Undefined, args)
}

/// (line, column), 1-based, of a UTF-16 offset in `src`.
fn line_col(src: &str, pos: u32) -> (u32, u32) {
    let (mut line, mut col) = (1u32, 1u32);
    for (i, u) in src.encode_utf16().enumerate() {
        if i as u32 >= pos {
            break;
        }
        if u == u16::from(b'\n') {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }
    (line, col)
}

/// Reports an uncaught exception: an ErrorEvent at the global, then (not canceled) a `worker-error` to the
/// page. `src`: the script whose evaluation threw (a SyntaxError's position is found by re-parsing it).
fn report(vm: &mut Vm, err: &Value, src: Option<&str>) {
    if vm.terminated {
        return;
    }
    let text = vm.error_string(err);
    let msg = format!("Uncaught {text}");
    let (url, kind, id, owner) = with_w(|w| (w.url.clone(), w.kind, w.id, w.owner)).unwrap_or((String::new(), Kind::Classic, 0, 0));
    let (mut line, mut col) = (0u32, 0u32);
    if let (Some(src), true) = (src, text.starts_with("SyntaxError")) {
        let u: Vec<u16> = src.encode_utf16().collect();
        let parsed = match kind {
            Kind::Classic => js_core::parser::parse_script(&u).err(),
            Kind::Module => js_core::parser::parse_module(&u).err(),
        };
        if let Some(pe) = parsed {
            (line, col) = line_col(src, pe.pos);
        }
    }
    let mut not_canceled = true;
    if !REPORTING.with(|r| r.replace(true)) {
        let args = [s(&msg), s(&url), Value::Number(line as f64), Value::Number(col as f64), err.clone()];
        let saved = vm.budget;
        vm.budget = Some(crate::js::TASK_BUDGET);
        if let Ok(v) = call_global(vm, "__w_error", &args) {
            not_canceled = !matches!(v, Value::Bool(false));
        }
        if !vm.terminated {
            vm.budget = saved;
        }
        REPORTING.with(|r| r.set(false));
    }
    if not_canceled {
        eprintln!("[aether-worker] {msg} ({url}:{line}:{col})");
        crate::js::broadcast::post(owner, "worker-error", json!({ "id": id, "message": msg, "filename": url, "lineno": line, "colno": col }).to_string());
    }
}

/// One task: `f` under the task budget, an uncaught exception reported, then a microtask checkpoint whose
/// exceptions are reported too.
fn task(vm: &mut Vm, src: Option<&str>, f: impl FnOnce(&mut Vm) -> JsResult<()>) {
    if killed() {
        return;
    }
    vm.loop_now = elapsed_ms();
    vm.budget = Some(crate::js::TASK_BUDGET);
    vm.terminated = false;
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if let Err(e) = f(vm) {
            if vm.terminated {
                if !killed() {
                    let m = vm.error_string(&e);
                    eprintln!("[aether-worker] task terminated: {m}");
                    let (id, owner, url) = with_w(|w| (w.id, w.owner, w.url.clone())).unwrap_or((0, 0, String::new()));
                    let msg = format!("Uncaught RangeError: the worker task was terminated ({m})");
                    crate::js::broadcast::post(owner, "worker-error", json!({ "id": id, "message": msg, "filename": url, "lineno": 0, "colno": 0 }).to_string());
                }
                return;
            }
            report(vm, &e, src);
        }
        loop {
            match vm.run_jobs() {
                Ok(()) => break,
                Err(e) => {
                    if vm.terminated {
                        break;
                    }
                    report(vm, &e, None);
                }
            }
        }
    }));
    if r.is_err() {
        // A binding bug must cost the worker, not the browser: the worker closes, the page hears an error.
        let (id, owner, url) = with_w(|w| {
            w.closing = true;
            (w.id, w.owner, w.url.clone())
        })
        .unwrap_or((0, 0, String::new()));
        crate::ledger::record_js("worker-engine-panicked");
        crate::js::broadcast::post(owner, "worker-error", json!({ "id": id, "message": "Uncaught Error: the worker's engine failed", "filename": url, "lineno": 0, "colno": 0 }).to_string());
    }
}

/// Fires the realm's timers due at the worker's clock, one generation, each its own task.
fn fire_due_timers(vm: &mut Vm) {
    let now = elapsed_ms();
    let mut due: Vec<(f64, u64, u32)> = vm.timers.iter().filter(|t| t.due <= now).map(|t| (t.due, t.seq, t.id)).collect();
    due.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    due.truncate(MAX_FIRES_PER_PASS);
    for (_, _, id) in due {
        if killed() || with_w(|w| w.closing).unwrap_or(true) {
            return;
        }
        let Some(i) = vm.timers.iter().position(|t| t.id == id) else { continue };
        let (callback, args) = match vm.timers[i].interval {
            Some(period) => {
                let seq = vm.timers[i].seq;
                vm.timers[i].due = now + period;
                vm.timers[i].seq = seq + (1u64 << 32);
                (vm.timers[i].callback.clone(), vm.timers[i].args.clone())
            }
            None => {
                let t = vm.timers.remove(i);
                (t.callback, t.args)
            }
        };
        task(vm, None, |vm| vm.call(&callback, &Value::Undefined, &args).map(|_| ()));
    }
}

fn next_due(vm: &Vm) -> Option<f64> {
    vm.timers.iter().map(|t| t.due).fold(None, |a: Option<f64>, d| Some(a.map_or(d, |a| a.min(d))))
}

/// Builds the DedicatedWorkerGlobalScope realm.
fn build(sp: &Spawn, shared: &Shared) -> Vm {
    let ctx = rebase(sp.ctx.clone(), &sp.url);
    let mut vm = Vm::new(Box::new(WorkerHost { url: sp.url.clone(), modules: 0, ctx }));
    // terminate() interrupts the running script at its next instruction (js_core vm/interp.rs run_inner).
    vm.interrupt = Some(shared.kill.clone());
    js_core::builtins::host::install_host_globals(&mut vm);
    super::realm::install_shared(&mut vm);
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__w_post", 2, post_native as NativeFn),
        ("__w_close", 0, close_native),
        ("__w_import", 1, import_native),
        ("__w_now", 0, now_native),
        ("__w_uncaught", 1, uncaught_native),
        ("__w_forward_error", 4, forward_error_native),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    vm.budget = Some(crate::js::TASK_BUDGET);
    for (label, src) in [("worker-events", EVENTS), ("serialize", crate::js::broadcast::SERIALIZE)] {
        if let Err(e) = vm.run_script_str(src) {
            eprintln!("[aether-worker] {label} prelude failed: {}", vm.error_string(&e));
        }
    }
    crate::api::platform::init_worker(&mut vm);
    for (label, src) in [("worker-fetch", super::realm::FETCH), ("cache-shapes", crate::js::cache_api::CACHE_SHAPES)] {
        if let Err(e) = vm.run_script_str(src) {
            eprintln!("[aether-worker] {label} prelude failed: {}", vm.error_string(&e));
        }
    }
    crate::js::ports::install_into(&mut vm);
    if let Err(e) = vm.run_script_str(SCOPE) {
        eprintln!("[aether-worker] scope prelude failed: {}", vm.error_string(&e));
    }
    // AETHERWORKERS2 (SR125): the inbox dispatcher and BroadcastChannel over this thread's serial, and `Worker`
    // itself — a nested worker's spawn path reads this worker's captured facts (`creator`).
    crate::js::broadcast::install_worker(&mut vm);
    crate::js::worker::install(&mut vm);
    let info = json!({
        "url": sp.url,
        "name": sp.name,
        "type": if sp.kind == Kind::Module { "module" } else { "classic" },
        "navigator": serde_json::from_str::<serde_json::Value>(&sp.navigator).unwrap_or(serde_json::Value::Null),
    })
    .to_string();
    if let Err(e) = call_global(&mut vm, "__w_init", &[s(&info)]) {
        eprintln!("[aether-worker] init failed: {}", vm.error_string(&e));
    }
    let _ = vm.run_jobs();
    vm
}

fn worker_main(sp: Spawn, rx: Receiver<Cmd>, shared: Arc<Shared>) {
    *threads_map().entry(sp.doc).or_default() += 1;
    crate::js::ports::set_worker_owner(Some(sp.id));
    super::realm::enter(super::realm::RealmCtx { key: sp.storage_key.clone(), scope: String::new(), script_url: sp.url.clone() });
    // The worker's own inbox serial: what `js::broadcast::post` sends to it becomes a `Cmd::Inbox` task here.
    let my_id = sp.id;
    let serial = crate::js::broadcast::enter_worker(Box::new(move |kind: &str, payload: String| send(my_id, Cmd::Inbox { kind: kind.to_string(), payload })));
    W.with(|w| {
        *w.borrow_mut() = Some(WState {
            id: sp.id,
            doc: sp.doc,
            serial,
            owner: sp.owner,
            storage_key: sp.storage_key.clone(),
            secure: sp.secure,
            ctx: rebase(sp.ctx.clone(), &sp.url),
            url: sp.url.clone(),
            kind: sp.kind,
            shared: shared.clone(),
            closing: false,
            start: Instant::now(),
        })
    });
    let mut vm = build(&sp, &shared);
    let (kind, url, source) = (sp.kind, sp.url.clone(), sp.source.clone());
    task(&mut vm, Some(&source), |vm| match kind {
        Kind::Classic => vm.run_script_str(&source).map(|_| ()),
        Kind::Module => vm.run_module(&url, &source).map(|_| ()),
    });
    shared.pending.fetch_sub(1, Ordering::SeqCst);
    loop {
        if shared.kill.load(Ordering::SeqCst) || with_w(|w| w.closing).unwrap_or(true) {
            break;
        }
        let now = elapsed_ms();
        let next = next_due(&vm);
        shared.timer_soon.store(next.is_some_and(|d| d - now <= SOON_MS), Ordering::Relaxed);
        let got = match next {
            Some(d) => rx.recv_timeout(Duration::from_secs_f64(((d - now).max(0.0)) / 1000.0)),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        match got {
            Ok(Cmd::Stop) | Err(RecvTimeoutError::Disconnected) => break,
            Ok(Cmd::Message { data, ports }) => {
                task(&mut vm, None, |vm| call_global(vm, "__w_deliver", &[s(&data), s(&ports)]).map(|_| ()));
                shared.pending.fetch_sub(1, Ordering::SeqCst);
            }
            Ok(Cmd::Port { port, data, ports }) => {
                task(&mut vm, None, |vm| call_global(vm, "__port_deliver", &[Value::Number(port as f64), s(&data), s(&ports)]).map(|_| ()));
                shared.pending.fetch_sub(1, Ordering::SeqCst);
            }
            Ok(Cmd::Inbox { kind, payload }) => {
                task(&mut vm, None, |vm| call_global(vm, "__inbox", &[s(&kind), s(&payload)]).map(|_| ()));
                shared.pending.fetch_sub(1, Ordering::SeqCst);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        fire_due_timers(&mut vm);
    }
    shared.kill.store(true, Ordering::SeqCst);
    shared.timer_soon.store(false, Ordering::Relaxed);
    {
        let mut w = workers();
        if w.get(&sp.id).is_some_and(|h| Arc::ptr_eq(&h.shared, &shared)) {
            w.remove(&sp.id);
        }
    }
    crate::js::ports::owner_gone(crate::js::ports::Owner::Worker(sp.id));
    crate::js::ports::set_worker_owner(None);
    // Its nested workers lose their owner; its serial retires (connections, locks, channels).
    terminate_children(sp.id);
    crate::js::broadcast::leave_worker();
    W.with(|w| *w.borrow_mut() = None);
    let mut t = threads_map();
    if let Some(n) = t.get_mut(&sp.doc) {
        *n = n.saturating_sub(1);
        if *n == 0 {
            t.remove(&sp.doc);
        }
    }
}

/// DOMException, Event / EventTarget / MessageEvent / ErrorEvent, the global as a DedicatedWorkerGlobalScope,
/// and `__net_handlers` (the event-handler IDL attributes, the same shape the page's `js::net` defines).
const EVENTS: &str = r#"
(function () {
    var G = globalThis;
    var hide = function (n, v) { Object.defineProperty(G, n, { value: v, writable: true, configurable: true, enumerable: false }); };
    if (typeof DOMException !== 'function') {
        var CODES = { IndexSizeError: 1, HierarchyRequestError: 3, WrongDocumentError: 4, InvalidCharacterError: 5, NotFoundError: 8,
            NotSupportedError: 9, InvalidStateError: 11, SyntaxError: 12, InvalidAccessError: 15, SecurityError: 18, NetworkError: 19,
            AbortError: 20, TimeoutError: 23, DataCloneError: 25 };
        class DOMException extends Error {
            constructor(message, name) {
                super(message === undefined ? '' : String(message));
                Object.defineProperty(this, 'name', { value: name === undefined ? 'Error' : String(name), writable: true, configurable: true });
            }
            get code() { return CODES[this.name] || 0; }
        }
        Object.defineProperty(DOMException.prototype, Symbol.toStringTag, { value: 'DOMException', configurable: true });
        hide('DOMException', DOMException);
    }

    var TRUSTED = new WeakSet();
    var EV = new WeakMap();
    var ev = function (e) { var s = (e !== null && typeof e === 'object') ? EV.get(e) : undefined; if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    class Event {
        constructor(type, init) {
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'Event': 1 argument required, but only 0 present."); }
            init = init || {};
            EV.set(this, { type: String(type), bubbles: !!init.bubbles, cancelable: !!init.cancelable, composed: !!init.composed,
                canceled: false, stop: false, stopNow: false, target: null, current: null, phase: 0, ts: __w_now(), dispatching: false });
        }
        get type() { return ev(this).type; }
        get bubbles() { return ev(this).bubbles; }
        get cancelable() { return ev(this).cancelable; }
        get composed() { return ev(this).composed; }
        get target() { return ev(this).target; }
        get srcElement() { return ev(this).target; }
        get currentTarget() { return ev(this).current; }
        get eventPhase() { return ev(this).phase; }
        get timeStamp() { return ev(this).ts; }
        get isTrusted() { return TRUSTED.has(this); }
        get defaultPrevented() { return ev(this).canceled; }
        get returnValue() { return !ev(this).canceled; }
        set returnValue(v) { if (!v) { this.preventDefault(); } }
        get cancelBubble() { return ev(this).stop; }
        set cancelBubble(v) { if (v) { ev(this).stop = true; } }
        preventDefault() { var s = ev(this); if (s.cancelable) { s.canceled = true; } }
        stopPropagation() { ev(this).stop = true; }
        stopImmediatePropagation() { var s = ev(this); s.stop = true; s.stopNow = true; }
        composedPath() { var s = ev(this); return s.current ? [s.current] : []; }
    }
    [['NONE', 0], ['CAPTURING_PHASE', 1], ['AT_TARGET', 2], ['BUBBLING_PHASE', 3]].forEach(function (c) {
        Object.defineProperty(Event, c[0], { value: c[1], enumerable: true });
        Object.defineProperty(Event.prototype, c[0], { value: c[1], enumerable: true });
    });
    var MSG = new WeakMap();
    class MessageEvent extends Event {
        constructor(type, init) {
            super(type, init);
            init = init || {};
            MSG.set(this, { data: init.data === undefined ? null : init.data, origin: init.origin === undefined ? '' : String(init.origin),
                lastEventId: init.lastEventId === undefined ? '' : String(init.lastEventId), source: init.source === undefined ? null : init.source,
                ports: Object.freeze(Array.from(init.ports || [])) });
        }
        get data() { return MSG.get(this).data; }
        get origin() { return MSG.get(this).origin; }
        get lastEventId() { return MSG.get(this).lastEventId; }
        get source() { return MSG.get(this).source; }
        get ports() { return MSG.get(this).ports; }
    }
    var ERR = new WeakMap();
    class ErrorEvent extends Event {
        constructor(type, init) {
            super(type, init);
            init = init || {};
            ERR.set(this, { message: init.message === undefined ? '' : String(init.message), filename: init.filename === undefined ? '' : String(init.filename),
                lineno: (init.lineno >>> 0), colno: (init.colno >>> 0), error: init.error === undefined ? undefined : init.error });
        }
        get message() { return ERR.get(this).message; }
        get filename() { return ERR.get(this).filename; }
        get lineno() { return ERR.get(this).lineno; }
        get colno() { return ERR.get(this).colno; }
        get error() { return ERR.get(this).error; }
    }

    var LS = new WeakMap();
    var listeners = function (t) { var m = LS.get(t); if (!m) { m = Object.create(null); LS.set(t, m); } return m; };
    var capOf = function (o) { return typeof o === 'boolean' ? o : !!(o && typeof o === 'object' && o.capture); };
    class EventTarget {
        constructor() {}
        addEventListener(type, cb, opts) {
            if (cb === null || cb === undefined) { return; }
            var t = (this === undefined || this === null) ? G : this;
            var capture = capOf(opts);
            var once = !!(opts && typeof opts === 'object' && opts.once);
            var m = listeners(t);
            type = String(type);
            var l = m[type] || (m[type] = []);
            for (var i = 0; i < l.length; i++) { if (l[i].cb === cb && l[i].capture === capture) { return; } }
            var rec = { cb: cb, capture: capture, once: once, removed: false };
            l.push(rec);
            if (opts && typeof opts === 'object' && opts.signal && typeof opts.signal.addEventListener === 'function') {
                if (opts.signal.aborted) { rec.removed = true; l.splice(l.indexOf(rec), 1); return; }
                opts.signal.addEventListener('abort', function () { EventTarget.prototype.removeEventListener.call(t, type, cb, opts); });
            }
        }
        removeEventListener(type, cb, opts) {
            var t = (this === undefined || this === null) ? G : this;
            var l = listeners(t)[String(type)];
            if (!l) { return; }
            var capture = capOf(opts);
            for (var i = 0; i < l.length; i++) {
                if (l[i].cb === cb && l[i].capture === capture) { l[i].removed = true; l.splice(i, 1); return; }
            }
        }
        dispatchEvent(e) {
            var s = ev(e);
            if (s.dispatching) { throw new DOMException("Failed to execute 'dispatchEvent' on 'EventTarget': The event is already being dispatched.", 'InvalidStateError'); }
            var t = (this === undefined || this === null) ? G : this;
            s.dispatching = true; s.target = t; s.current = t; s.phase = 2;
            var l = (listeners(t)[s.type] || []).slice();
            for (var i = 0; i < l.length; i++) {
                var x = l[i];
                if (x.removed) { continue; }
                if (x.once) { EventTarget.prototype.removeEventListener.call(t, s.type, x.cb, x.capture); }
                try {
                    if (typeof x.cb === 'function') { x.cb.call(t, e); }
                    else if (x.cb && typeof x.cb.handleEvent === 'function') { x.cb.handleEvent(e); }
                } catch (err) { __w_uncaught(err); }
                if (s.stopNow) { break; }
            }
            s.dispatching = false; s.phase = 0; s.current = null;
            return !s.canceled;
        }
    }
    [Event, MessageEvent, ErrorEvent, EventTarget].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(C.name, C);
    });

    // The global: DedicatedWorkerGlobalScope → WorkerGlobalScope → EventTarget.
    class WorkerGlobalScope extends EventTarget {
        constructor() { throw new TypeError('Illegal constructor'); }
    }
    class DedicatedWorkerGlobalScope extends WorkerGlobalScope {
        constructor() { throw new TypeError('Illegal constructor'); }
    }
    Object.defineProperty(WorkerGlobalScope.prototype, Symbol.toStringTag, { value: 'WorkerGlobalScope', configurable: true });
    Object.defineProperty(DedicatedWorkerGlobalScope.prototype, Symbol.toStringTag, { value: 'DedicatedWorkerGlobalScope', configurable: true });
    hide('WorkerGlobalScope', WorkerGlobalScope);
    hide('DedicatedWorkerGlobalScope', DedicatedWorkerGlobalScope);
    try { Object.setPrototypeOf(G, DedicatedWorkerGlobalScope.prototype); } catch (e) {}
    if (typeof G.addEventListener !== 'function') {
        ['addEventListener', 'removeEventListener', 'dispatchEvent'].forEach(function (n) { hide(n, EventTarget.prototype[n]); });
    }

    hide('__w_trust', function (e) { TRUSTED.add(e); return e; });
    hide('__net_handlers', function (proto, names) {
        names.forEach(function (n) {
            var key = '__on' + n, wkey = '__onw' + n;
            Object.defineProperty(proto, 'on' + n, {
                configurable: true, enumerable: true,
                get: function () { return this[key] === undefined ? null : this[key]; },
                set: function (f) {
                    var v = (typeof f === 'function' || (f !== null && typeof f === 'object')) ? f : null;
                    Object.defineProperty(this, key, { value: v, writable: true, configurable: true, enumerable: false });
                    if (v !== null && !this[wkey]) {
                        var self = this;
                        var w = function (e) {
                            var h = self[key];
                            if (typeof h === 'function') {
                                var r = h.call(self, e);
                                if (r === false) { e.preventDefault(); }
                            }
                        };
                        Object.defineProperty(this, wkey, { value: w, writable: true, configurable: true, enumerable: false });
                        EventTarget.prototype.addEventListener.call(this, n, w);
                    }
                },
            });
        });
    });
})();
"#;

/// The DedicatedWorkerGlobalScope members (after EVENTS, the serializer, the platform prelude, FETCH, the
/// Cache API shapes and the ports prelude).
const SCOPE: &str = r#"
(function () {
    var G = globalThis;
    var hide = function (n, v) { Object.defineProperty(G, n, { value: v, writable: true, configurable: true, enumerable: false }); };
    var INFO = { url: '', name: '', type: 'classic', navigator: null };
    var trusted = function (e) { __w_trust(e); return e; };

    // The handler attributes live on the global's prototypes; if the engine refused to re-parent the global,
    // they go on the global itself.
    var GP = (G instanceof DedicatedWorkerGlobalScope) ? DedicatedWorkerGlobalScope.prototype : G;
    var WP = (G instanceof WorkerGlobalScope) ? WorkerGlobalScope.prototype : G;
    __net_handlers(GP, ['message', 'messageerror']);
    (function () {
        // WorkerGlobalScope.onerror is an OnErrorEventHandler: (message, source, lineno, colno, error); true cancels.
        var h = null, wired = false;
        Object.defineProperty(WP, 'onerror', {
            configurable: true, enumerable: true,
            get: function () { return h; },
            set: function (f) {
                h = (typeof f === 'function' || (f !== null && typeof f === 'object')) ? f : null;
                if (h !== null && !wired) {
                    wired = true;
                    EventTarget.prototype.addEventListener.call(G, 'error', function (e) {
                        if (typeof h !== 'function') { return; }
                        var r = (e instanceof ErrorEvent) ? h.call(G, e.message, e.filename, e.lineno, e.colno, e.error) : h.call(G, e);
                        if (r === true) { e.preventDefault(); }
                    });
                }
            },
        });
    })();
    // `__net_fire(target, iface, type, init, cancelable)`: the page binding's trusted-event helper, for the
    // preludes both realms run (BroadcastChannel, IndexedDB, Web Locks, Worker). Defined after js::ports'
    // prelude, which reads its absence as "a worker realm".
    hide('__net_fire', function (t, iface, type, init, cancelable) {
        var C = (typeof G[iface] === 'function' && G[iface].prototype instanceof Event) ? G[iface] : Event;
        var d = {};
        if (init !== null && typeof init === 'object') { for (var k in init) { d[k] = init[k]; } }
        d.cancelable = !!cancelable;
        return t.dispatchEvent(trusted(new C(String(type), d)));
    });
    hide('self', G);
    hide('postMessage', function postMessage(message, transfer) {
        if (arguments.length < 1) { throw new TypeError("Failed to execute 'postMessage' on 'DedicatedWorkerGlobalScope': 1 argument required, but only 0 present."); }
        var pkt = __msg_pack(message, __msg_xfer(transfer, 'DedicatedWorkerGlobalScope'));
        __w_post(pkt.data, pkt.ports);
    });
    hide('close', function close() { __w_close(); });
    hide('importScripts', function importScripts() {
        if (INFO.type === 'module') { throw new TypeError("Failed to execute 'importScripts' on 'WorkerGlobalScope': Module scripts don't support importScripts()."); }
        var urls = [];
        for (var i = 0; i < arguments.length; i++) {
            var u;
            try { u = new URL(String(arguments[i]), INFO.url).href; }
            catch (e) { throw new DOMException("Failed to execute 'importScripts' on 'WorkerGlobalScope': The URL '" + arguments[i] + "' is invalid.", 'SyntaxError'); }
            urls.push(u);
        }
        urls.forEach(function (u) {
            var r = __w_import(u);
            if (typeof r === 'string') { throw new DOMException(r, 'NetworkError'); }
        });
    });
    ['DOMParser', 'XMLSerializer'].forEach(function (n) { try { delete G[n]; } catch (e) {} });

    // ---- Blob / File (the serializer reads their bytes through __blob_bytes)
    var BYTES = new WeakMap();
    var partBytes = function (p) {
        if (p instanceof Blob) { return BYTES.get(p); }
        if (p instanceof ArrayBuffer) { return new Uint8Array(p.slice(0)); }
        if (ArrayBuffer.isView(p)) { return new Uint8Array(p.buffer.slice(p.byteOffset, p.byteOffset + p.byteLength)); }
        return new TextEncoder().encode(String(p));
    };
    class Blob {
        constructor(parts, opts) {
            var chunks = Array.from(parts || []).map(partBytes), n = 0;
            chunks.forEach(function (c) { n += c.length; });
            var out = new Uint8Array(n), at = 0;
            chunks.forEach(function (c) { out.set(c, at); at += c.length; });
            BYTES.set(this, out);
            var t = opts && opts.type !== undefined ? String(opts.type) : '';
            Object.defineProperty(this, '__type', { value: /^[\x20-\x7e]*$/.test(t) ? t.toLowerCase() : '', configurable: true });
        }
        get size() { return BYTES.get(this).length; }
        get type() { return this.__type; }
        slice(start, end, type) {
            var b = BYTES.get(this), n = b.length;
            var rel = function (x, d) { if (x === undefined) { return d; } x = Math.trunc(Number(x)) || 0; return x < 0 ? Math.max(n + x, 0) : Math.min(x, n); };
            var a = rel(start, 0), z = rel(end, n);
            return new Blob([b.slice(a, Math.max(a, z))], { type: type === undefined ? '' : type });
        }
        arrayBuffer() { return Promise.resolve(BYTES.get(this).slice(0).buffer); }
        bytes() { return Promise.resolve(BYTES.get(this).slice(0)); }
        text() { return Promise.resolve(new TextDecoder().decode(BYTES.get(this))); }
    }
    class File extends Blob {
        constructor(bits, name, opts) {
            super(bits, opts);
            Object.defineProperty(this, '__name', { value: String(name), configurable: true });
            Object.defineProperty(this, '__lm', { value: opts && opts.lastModified !== undefined ? Number(opts.lastModified) : Date.now(), configurable: true });
        }
        get name() { return this.__name; }
        get lastModified() { return this.__lm; }
    }
    [Blob, File].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(C.name, C);
    });
    hide('__blob_bytes', function (b) { return BYTES.get(b) || new Uint8Array(0); });

    // ---- XMLHttpRequest over the worker's fetch native (one exchange; an async request's events are tasks)
    var X = new WeakMap();
    var xs = function (o) { var s = X.get(o); if (!s) { throw new TypeError('Illegal invocation'); } return s; };
    var ise = function (m) { return new DOMException(m, 'InvalidStateError'); };
    var fire = function (t, type) { t.dispatchEvent(trusted(new Event(type))); };
    class XMLHttpRequest extends EventTarget {
        constructor() {
            super();
            X.set(this, { rs: 0, method: 'GET', url: '', async: true, headers: [], resp: null, status: 0, statusText: '', respUrl: '',
                rh: [], type: '', sent: false, aborted: false, timeout: 0, creds: false });
        }
        get readyState() { return xs(this).rs; }
        get status() { return xs(this).status; }
        get statusText() { return xs(this).statusText; }
        get responseURL() { return xs(this).respUrl; }
        get responseType() { return xs(this).type; }
        set responseType(v) { var s = xs(this); if (s.rs === 4) { throw ise("Failed to set the 'responseType' property on 'XMLHttpRequest': The response type cannot be set if the object's state is DONE."); } if (['', 'text', 'json', 'arraybuffer', 'blob'].indexOf(String(v)) >= 0) { s.type = String(v); } }
        get timeout() { return xs(this).timeout; }
        set timeout(v) { xs(this).timeout = Number(v) >>> 0; }
        get withCredentials() { return xs(this).creds; }
        set withCredentials(v) { xs(this).creds = !!v; }
        open(method, url, async) {
            var s = xs(this);
            var u;
            try { u = new URL(String(url), INFO.url).href; } catch (e) { throw new DOMException("Failed to execute 'open' on 'XMLHttpRequest': Invalid URL", 'SyntaxError'); }
            s.method = String(method).toUpperCase(); s.url = u; s.async = async === undefined ? true : !!async;
            s.headers = []; s.resp = null; s.status = 0; s.statusText = ''; s.rh = []; s.sent = false; s.aborted = false; s.rs = 1;
            fire(this, 'readystatechange');
        }
        setRequestHeader(k, v) {
            var s = xs(this);
            if (s.rs !== 1 || s.sent) { throw ise("Failed to execute 'setRequestHeader' on 'XMLHttpRequest': The object's state must be OPENED."); }
            s.headers.push(String(k), String(v));
        }
        send(body) {
            var s = xs(this), self = this;
            if (s.rs !== 1 || s.sent) { throw ise("Failed to execute 'send' on 'XMLHttpRequest': The object's state must be OPENED."); }
            s.sent = true;
            var run = function () {
                var b;
                if (body !== undefined && body !== null && s.method !== 'GET' && s.method !== 'HEAD') {
                    b = body instanceof Blob ? __blob_bytes(body).slice(0).buffer : body instanceof ArrayBuffer ? body.slice(0)
                        : ArrayBuffer.isView(body) ? body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength) : __sw_utf8_encode(String(body));
                }
                var a;
                try { a = __sw_fetch(s.method, s.url, s.headers, b, 'cors'); }
                catch (e) {
                    s.rs = 4; s.status = 0;
                    if (!s.async) { throw new DOMException("Failed to execute 'send' on 'XMLHttpRequest': " + String(e && e.message || e), 'NetworkError'); }
                    fire(self, 'readystatechange'); fire(self, 'error'); fire(self, 'loadend');
                    return;
                }
                s.status = a[0]; s.statusText = a[1]; s.respUrl = a[2]; s.resp = a[4];
                s.rh = [];
                for (var i = 0; i + 1 < a[3].length; i += 2) { s.rh.push([String(a[3][i]).toLowerCase(), a[3][i + 1]]); }
                s.rs = 2; fire(self, 'readystatechange');
                s.rs = 3; fire(self, 'readystatechange');
                s.rs = 4; fire(self, 'readystatechange');
                fire(self, 'load'); fire(self, 'loadend');
            };
            if (s.async) {
                fire(this, 'loadstart');
                setTimeout(function () { if (!s.aborted) { run(); } }, 0);
            } else {
                run();
            }
        }
        abort() {
            var s = xs(this);
            s.aborted = true;
            if (s.sent && s.rs >= 1 && s.rs < 4) { s.rs = 4; fire(this, 'readystatechange'); fire(this, 'abort'); fire(this, 'loadend'); }
            s.rs = 0; s.sent = false;
        }
        getResponseHeader(n) {
            var k = String(n).toLowerCase();
            var v = xs(this).rh.filter(function (p) { return p[0] === k; }).map(function (p) { return p[1]; });
            return v.length ? v.join(', ') : null;
        }
        getAllResponseHeaders() { return xs(this).rh.map(function (p) { return p[0] + ': ' + p[1] + '\r\n'; }).join(''); }
        overrideMimeType() {}
        get responseText() {
            var s = xs(this);
            if (s.type !== '' && s.type !== 'text') { throw ise("Failed to read the 'responseText' property from 'XMLHttpRequest': The value is only accessible if the object's 'responseType' is '' or 'text'."); }
            return s.resp ? __sw_utf8_decode(s.resp) : '';
        }
        get response() {
            var s = xs(this);
            if (s.type === '' || s.type === 'text') { return this.responseText; }
            if (s.rs !== 4 || !s.resp) { return null; }
            if (s.type === 'json') { try { return JSON.parse(__sw_utf8_decode(s.resp)); } catch (e) { return null; } }
            if (s.type === 'arraybuffer') { return s.resp.slice(0); }
            return new Blob([s.resp]);
        }
    }
    __net_handlers(XMLHttpRequest.prototype, ['readystatechange', 'loadstart', 'progress', 'load', 'loadend', 'error', 'abort', 'timeout']);
    [['UNSENT', 0], ['OPENED', 1], ['HEADERS_RECEIVED', 2], ['LOADING', 3], ['DONE', 4]].forEach(function (c) {
        Object.defineProperty(XMLHttpRequest, c[0], { value: c[1], enumerable: true });
        Object.defineProperty(XMLHttpRequest.prototype, c[0], { value: c[1], enumerable: true });
    });
    Object.defineProperty(XMLHttpRequest.prototype, Symbol.toStringTag, { value: 'XMLHttpRequest', configurable: true });
    hide('XMLHttpRequest', XMLHttpRequest);

    var origin0 = Date.now() - __w_now();
    hide('performance', { now: function () { return __w_now(); }, timeOrigin: origin0, toJSON: function () { return { timeOrigin: origin0 }; } });

    hide('__w_init', function (j) {
        INFO = JSON.parse(j);
        var u = new URL(INFO.url);
        var loc = Object.create(null);
        ['href', 'origin', 'protocol', 'host', 'hostname', 'port', 'pathname', 'search', 'hash'].forEach(function (k) {
            Object.defineProperty(loc, k, { value: u[k], enumerable: true });
        });
        loc.toString = function () { return u.href; };
        Object.defineProperty(loc, Symbol.toStringTag, { value: 'WorkerLocation' });
        hide('location', Object.freeze(loc));
        var n = INFO.navigator || {};
        var nav = {
            userAgent: n.userAgent || '', appName: n.appName || 'Netscape', appVersion: n.appVersion || '', appCodeName: 'Mozilla',
            product: 'Gecko', platform: n.platform || '', language: n.language || 'en-US', languages: Object.freeze((n.languages || [n.language || 'en-US']).slice()),
            onLine: true, hardwareConcurrency: n.hardwareConcurrency || 1,
        };
        Object.defineProperty(nav, Symbol.toStringTag, { value: 'WorkerNavigator' });
        hide('navigator', Object.freeze(nav));
        hide('name', String(INFO.name));
        var cs = __cache_shapes();
        Object.defineProperty(G, 'caches', {
            get: function () { return __cache('secure', '{}') === true ? cs : undefined; },
            configurable: true, enumerable: true,
        });
    });
    hide('__w_error', function (message, filename, lineno, colno, error) {
        return G.dispatchEvent(trusted(new ErrorEvent('error', { message: message, filename: filename, lineno: lineno, colno: colno, error: error, cancelable: true })));
    });
    hide('__w_deliver', function (text, csv) { __msg_dispatch(G, text, csv); });
})();
"#;
