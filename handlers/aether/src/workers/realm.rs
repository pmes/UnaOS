//! The service-worker realm (AETHERSTORAGE, LEDGER SR94; Service Workers §4 ServiceWorkerGlobalScope).
//!
//! CHARTER: Aether — engine (the worker half of `workers::sw`).
//!
//! Each running service worker is one thread with its own `js_core` realm — no DOM, no page bindings — whose
//! global is the ServiceWorkerGlobalScope this module builds: `self`, `addEventListener` / `removeEventListener`
//! and the `on<event>` attributes, `skipWaiting()`, `clients` (`get`, `matchAll`, `claim`, a `Client` with
//! `postMessage`), `registration` (`scope`), `caches` (the same Cache API classes as the page, over the same disk
//! store, keyed by the worker's storage key), `fetch` / `Request` / `Response` / `Headers` (a synchronous
//! exchange through `api::fetch::http_sync`, answered as a settled promise), `importScripts`, `console`, the
//! timers on js_core's built-in event loop, `structuredClone` and the serializer `postMessage` uses.
//!
//! The thread runs [`Cmd`]s: `Event` dispatches an `install`, `activate`, `fetch` or `message` event as an
//! ExtendableEvent / FetchEvent / ExtendableMessageEvent and drives the realm's microtasks and timers until
//! every `waitUntil` promise (and a `fetch` event's `respondWith` promise) settles or [`EVENT_LIMIT`] passes.
//! A `fetch` event nobody answered with `respondWith` falls back to the network (`None`).
//!
//! AETHERSW (LEDGER SR104): a `fetch` event's answer carries its response TYPE ([`SwResponse::kind`]) so the
//! caller's policy (`workers::sw::vet`) can refuse an opaque answer to a `navigate` / `cors` request; a rejected
//! `respondWith` (or `Response.error()`) answers kind `error` — a network error, never a silent fallback. The
//! worker's own `fetch` is a client of the worker's origin: a cross-origin `no-cors` request answers an
//! OPAQUE response (status 0, no headers; its body stays hidden from script and rides only to the page that
//! asked), a cross-origin `cors` one passes `security::cors::cors_check` against the worker's origin, and a
//! cross-origin `same-origin` one is refused — each refusal through `security::refusal`.

use js_core::vm::*;
use serde_json::{json, Value as J};
use std::cell::RefCell;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

/// How long one event may keep the worker busy before the page gives up on it (network fallback, a failed
/// install).
pub const EVENT_LIMIT: Duration = Duration::from_secs(30);

/// A response a `fetch` event produced.
#[derive(Debug, Clone)]
pub struct SwResponse {
    pub status: u16,
    pub status_text: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// The Response's type: `default` (constructed), `basic`, `cors`, `opaque`, or `error` (a network error:
    /// `Response.error()` or a rejected `respondWith`).
    pub kind: String,
}

/// How one event settled.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    /// A `fetch` event's answer (`None`: no `respondWith`, the network serves the request).
    pub response: Option<SwResponse>,
    /// `skipWaiting()` has been called in this worker.
    pub skip_waiting: bool,
}

/// What the worker thread is asked to do.
pub enum Cmd {
    /// Dispatch `install`/`activate`/`fetch`/`message` with its JSON payload; the reply carries the outcome, or
    /// the rejection / timeout message.
    Event(String, String, Sender<Result<Outcome, String>>),
    Stop,
}

/// What the realm knows about itself (its natives read it).
#[derive(Clone)]
pub struct RealmCtx {
    pub key: String,
    pub scope: String,
    pub script_url: String,
}

struct State {
    ctx: RealmCtx,
    skip_waiting: bool,
}

thread_local! {
    static STATE: RefCell<Option<State>> = const { RefCell::new(None) };
}

struct SwHost;

impl Host for SwHost {
    fn console(&mut self, level: u8, msg: &str) {
        let tag = ["log", "warn", "error", "debug", "info"].get(level as usize).copied().unwrap_or("log");
        eprintln!("[aether-sw] {tag}: {msg}");
    }
    fn now_ms(&mut self) -> f64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64() * 1000.0).unwrap_or(0.0)
    }
}

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = vm.arg(ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

fn ctx() -> RealmCtx {
    STATE.with(|s| s.borrow().as_ref().map(|s| s.ctx.clone())).unwrap_or(RealmCtx { key: String::new(), scope: String::new(), script_url: String::new() })
}

fn s(v: &str) -> Value {
    Value::String(js_core::string::JsStr::from_str(v))
}

/// `__sw_url(input, base)` → the resolved URL or null.
fn url_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let input = str_arg(vm, c, 0)?;
    let base = vm.arg(c, 1);
    let base = if base.is_nullish() { ctx().script_url } else { vm.to_string(&base)?.to_rust() };
    match html_core::url::Url::parse(&base).and_then(|b| b.join(&input)) {
        Ok(u) => Ok(s(&u.to_string())),
        Err(_) => Ok(Value::Null),
    }
}

/// `__sw_fetch(method, url, [k, v, …], body, mode)` → `[status, statusText, url, [k, v, …], ArrayBuffer, type]`;
/// a network error throws a TypeError ("Failed to fetch"). The worker is the request's client: a cross-origin
/// target is refused under `same-origin`, CORS-checked under `cors`, and answered opaque under `no-cors`.
fn fetch_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    use crate::security::origin::Origin;
    let method = str_arg(vm, c, 0)?;
    let url = str_arg(vm, c, 1)?;
    let mode = {
        let m = vm.arg(c, 4);
        if m.is_nullish() { "cors".to_string() } else { vm.to_string(&m)?.to_rust() }
    };
    // AETHERWORKERS3 (SR129): on a dedicated / shared worker thread the worker's OWN policy container governs
    // its fetches (`connect-src`, CSP3 §6.8.1) — the policies of its script response, or its creator's for a
    // local-scheme worker (`workers::dedicated::connect_allowed`). A service worker's realm has none here.
    if let Err(e) = super::dedicated::connect_allowed(&url) {
        return vm.throw_type(&format!("Failed to fetch: {}", crate::js::host::refusal_message(&e)));
    }
    let me = Origin::of(&ctx().script_url);
    let cross = !me.same_origin(&Origin::of(&url));
    if cross && mode == "same-origin" {
        let e = crate::security::refusal("sw-same-origin-mode", &url, &format!("Request mode is \"same-origin\" but the service worker ({}) fetched {url}.", me.serialize()), None);
        return vm.throw_type(&format!("Failed to fetch: {e}"));
    }
    let hv = vm.arg(c, 2);
    let mut flat = Vec::new();
    if let Value::Object(o) = hv {
        let len = vm.get(o, &PropertyKey::from_str("length"))?;
        let n = vm.to_number(&len)? as usize;
        for i in 0..n.min(512) {
            let e = vm.get(o, &PropertyKey::from_str(&i.to_string()))?;
            flat.push(vm.to_string(&e)?.to_rust());
        }
    }
    let headers: Vec<(String, String)> = flat.chunks(2).filter(|p| p.len() == 2).map(|p| (p[0].clone(), p[1].clone())).collect();
    let bv = vm.arg(c, 3);
    let body = crate::js::net::buffer_bytes(vm, &bv).unwrap_or_default();
    let req = crate::api::fetch::HttpReq { method, url: url.clone(), headers, body, redirect: "follow".into(), timeout_ms: 30_000 };
    match crate::api::fetch::http_sync(req) {
        Ok(r) => {
            let kind = if !cross || mode == "navigate" {
                "basic"
            } else if mode == "no-cors" {
                "opaque"
            } else {
                let pairs: Vec<(String, String)> = r.headers.chunks(2).filter(|p| p.len() == 2).map(|p| (p[0].to_ascii_lowercase(), p[1].clone())).collect();
                if let Err(e) = crate::security::cors::cors_check(&pairs, &me.serialize(), false) {
                    let e = crate::security::refusal("sw-cors", &url, &format!("Access to fetch at '{url}' from the service worker of origin '{}' has been blocked by CORS policy: {e}.", me.serialize()), None);
                    return vm.throw_type(&format!("Failed to fetch: {e}"));
                }
                "cors"
            };
            let opaque = kind == "opaque";
            let hs: Vec<Value> = if opaque { Vec::new() } else { r.headers.iter().map(|h| s(h)).collect() };
            let hl = vm.new_array(hs);
            // An opaque body rides hidden (`Response.__b`): script reads it as empty, the requester gets it.
            let buf = crate::js::net::array_buffer(vm, &r.body)?;
            let status = if opaque { 0.0 } else { r.status as f64 };
            let (st_text, r_url) = if opaque { (String::new(), String::new()) } else { (r.status_text, r.url) };
            let out = vm.new_array(vec![Value::Number(status), s(&st_text), s(&r_url), Value::Object(hl), Value::Object(buf), s(kind)]);
            Ok(Value::Object(out))
        }
        Err(e) => vm.throw_type(&format!("Failed to fetch: {e}")),
    }
}

/// `__sw_utf8_decode(buffer)` / `__sw_utf8_encode(string)`.
fn utf8_decode(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let v = vm.arg(c, 0);
    let b = crate::js::net::buffer_bytes(vm, &v).unwrap_or_default();
    Ok(s(&String::from_utf8_lossy(&b)))
}

fn utf8_encode(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let t = str_arg(vm, c, 0)?;
    Ok(Value::Object(crate::js::net::array_buffer(vm, t.as_bytes())?))
}

fn skip_waiting(_vm: &mut Vm, _c: &CallCtx) -> JsResult<Value> {
    STATE.with(|s| {
        if let Some(s) = s.borrow_mut().as_mut() {
            s.skip_waiting = true;
        }
    });
    Ok(Value::Undefined)
}

/// `__sw_clients(includeUncontrolled)` → JSON `[{id, url, type}]`.
fn clients_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let all = vm.arg(c, 0);
    let all = matches!(all, Value::Bool(true));
    let x = ctx();
    Ok(s(&crate::workers::sw::clients_of(&x.key, &x.scope, all).to_string()))
}

fn claim_native(_vm: &mut Vm, _c: &CallCtx) -> JsResult<Value> {
    let x = ctx();
    crate::workers::sw::claim(&x.key, &x.scope);
    Ok(Value::Undefined)
}

/// `__sw_client_post(id, serialized)`: a `message` event at that document's `navigator.serviceWorker`.
fn client_post(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let id = str_arg(vm, c, 0)?.parse::<u64>().unwrap_or(0);
    let data = str_arg(vm, c, 1)?;
    let x = ctx();
    let origin = crate::security::origin::Origin::of(&x.script_url).serialize();
    crate::js::broadcast::post(id, "sw-message", json!({ "data": data, "origin": origin, "scope": x.scope }).to_string());
    Ok(Value::Undefined)
}

/// `__sw_import(url)`: fetch and run one script in the realm (`importScripts`).
fn import_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let url = str_arg(vm, c, 0)?;
    let src = match crate::workers::sw::fetch_script(&url) {
        Ok(t) => t,
        Err(e) => return vm.throw_type(&format!("Failed to execute 'importScripts': {e}")),
    };
    vm.run_script_str(&src)?;
    Ok(Value::Undefined)
}

/// `__cache(op, json, body)` in the worker: the page binding's operations over the worker's storage key.
fn cache_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let x = ctx();
    // AETHERWORKERS (SR123): a dedicated worker of an opaque origin has no storage key (SecurityError), and
    // `caches` is a secure-context member of a worker realm as of a page (a service worker always is one).
    // AETHERWORKERS3 (SR129): a dedicated / shared worker's flag is its owner document's (a `data:` worker's
    // URL is never trustworthy, yet its `caches` is defined and refuses with SecurityError: no storage key).
    let secure = crate::storage::worker_client_secure().unwrap_or_else(|| crate::security::origin::is_potentially_trustworthy_url(&x.script_url));
    let key = (!x.key.is_empty()).then_some(x.key);
    crate::js::cache_api::run(vm, c, key, secure)
}

fn ledger_native(vm: &mut Vm, c: &CallCtx) -> JsResult<Value> {
    let n = str_arg(vm, c, 0)?;
    crate::ledger::record_js(&format!("sw:{n}"));
    Ok(Value::Undefined)
}

fn call_global(vm: &mut Vm, name: &str, args: &[Value]) -> Option<Value> {
    let g = vm.realm().global;
    let f = vm.get(g, &PropertyKey::from_str(name)).ok()?;
    if !vm.is_callable(&f) {
        return None;
    }
    match vm.call(&f, &Value::Undefined, args) {
        Ok(v) => Some(v),
        Err(e) => {
            let m = vm.error_string(&e);
            eprintln!("[aether-sw] {name} threw: {m}");
            None
        }
    }
}

/// AETHERWORKERS (SR123): the natives a dedicated worker's realm shares with this one — URL resolution,
/// `fetch` as a client of the realm's script origin, UTF-8, the Cache API over the realm's storage key, the
/// ledger. They read the thread's [`RealmCtx`], set by [`enter`].
pub(crate) fn install_shared(vm: &mut Vm) {
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__sw_url", 2, url_native as NativeFn),
        ("__sw_fetch", 5, fetch_native),
        ("__sw_utf8_decode", 1, utf8_decode),
        ("__sw_utf8_encode", 1, utf8_encode),
        ("__cache", 3, cache_native),
        ("__ledger", 1, ledger_native),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
}

/// Binds this thread's realm context (the natives of [`install_shared`] read it).
pub(crate) fn enter(rc: RealmCtx) {
    STATE.with(|s| *s.borrow_mut() = Some(State { ctx: rc, skip_waiting: false }));
}

/// Builds the realm and evaluates the worker script. Answers the realm, or the evaluation error.
fn build(rc: &RealmCtx, script: &str) -> Result<Vm, String> {
    let mut vm = Vm::new(Box::new(SwHost));
    js_core::builtins::host::install_host_globals(&mut vm);
    install_shared(&mut vm);
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__sw_skip_waiting", 0, skip_waiting as NativeFn),
        ("__sw_clients", 1, clients_native),
        ("__sw_claim", 0, claim_native),
        ("__sw_client_post", 2, client_post),
        ("__sw_import", 1, import_native),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    vm.budget = Some(crate::js::TASK_BUDGET);
    let scope_json = json!({ "scope": rc.scope, "scriptURL": rc.script_url }).to_string();
    for (label, src) in [
        ("serialize", crate::js::broadcast::SERIALIZE),
        ("sw-fetch", FETCH),
        ("sw-scope", SCOPE),
        ("cache-shapes", crate::js::cache_api::CACHE_SHAPES),
        ("caches", CACHES),
    ] {
        if let Err(e) = vm.run_script_str(src) {
            return Err(format!("{label} prelude: {}", vm.error_string(&e)));
        }
    }
    call_global(&mut vm, "__sw_init", &[s(&scope_json)]);
    vm.budget = Some(crate::js::TASK_BUDGET);
    vm.terminated = false;
    if let Err(e) = vm.run_script_str(script) {
        return Err(vm.error_string(&e));
    }
    let _ = vm.checkpoint();
    Ok(vm)
}

/// Dispatches one event and drives the realm until it settles.
fn run_event(vm: &mut Vm, kind: &str, payload: &str) -> Result<Option<SwResponse>, String> {
    vm.budget = Some(crate::js::TASK_BUDGET);
    vm.terminated = false;
    let id = call_global(vm, "__sw_dispatch", &[s(kind), s(payload)]).ok_or_else(|| format!("{kind} dispatch failed"))?;
    let start = Instant::now();
    loop {
        let _ = vm.checkpoint();
        let _ = vm.run_event_loop(16);
        let st = call_global(vm, "__sw_status", std::slice::from_ref(&id)).map(|v| vm.to_string(&v).map(|t| t.to_rust()).unwrap_or_default()).unwrap_or_default();
        let st: J = serde_json::from_str(&st).unwrap_or(J::Null);
        if st.get("done").and_then(J::as_bool) == Some(true) {
            let body_v = call_global(vm, "__sw_body", std::slice::from_ref(&id)).unwrap_or(Value::Undefined);
            if st.get("ok").and_then(J::as_bool) != Some(true) {
                return Err(st.get("err").and_then(J::as_str).unwrap_or("rejected").to_string());
            }
            let Some(r) = st.get("resp").filter(|r| !r.is_null()) else { return Ok(None) };
            let body = crate::js::net::buffer_bytes(vm, &body_v).unwrap_or_default();
            let headers = r
                .get("headers")
                .and_then(J::as_array)
                .map(|l| l.iter().filter_map(|kv| Some((kv.get(0)?.as_str()?.to_string(), kv.get(1)?.as_str()?.to_string()))).collect())
                .unwrap_or_default();
            return Ok(Some(SwResponse {
                status: r.get("status").and_then(J::as_u64).unwrap_or(200) as u16,
                status_text: r.get("statusText").and_then(J::as_str).unwrap_or("").to_string(),
                url: r.get("url").and_then(J::as_str).unwrap_or("").to_string(),
                headers,
                body,
                kind: r.get("type").and_then(J::as_str).unwrap_or("default").to_string(),
            }));
        }
        if vm.terminated || start.elapsed() > EVENT_LIMIT {
            return Err(format!("{kind} event did not settle"));
        }
        if vm.timers.is_empty() {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// Starts a worker thread over `script`. Answers its command channel once the script evaluated, or the
/// evaluation error (the registration's job then fails with a TypeError).
pub fn spawn(rc: RealmCtx, script: String) -> Result<Sender<Cmd>, String> {
    let (tx, rx) = channel::<Cmd>();
    let (ready_tx, ready_rx) = channel::<Result<(), String>>();
    std::thread::Builder::new()
        .name("aether-service-worker".into())
        .stack_size(8 << 20)
        .spawn(move || worker_main(rc, script, rx, ready_tx))
        .map_err(|e| e.to_string())?;
    match ready_rx.recv_timeout(EVENT_LIMIT) {
        Ok(Ok(())) => Ok(tx),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the service worker script did not finish evaluating".into()),
    }
}

fn worker_main(rc: RealmCtx, script: String, rx: Receiver<Cmd>, ready: Sender<Result<(), String>>) {
    STATE.with(|s| *s.borrow_mut() = Some(State { ctx: rc.clone(), skip_waiting: false }));
    let mut vm = match build(&rc, &script) {
        Ok(vm) => {
            let _ = ready.send(Ok(()));
            vm
        }
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Stop => break,
            Cmd::Event(kind, payload, reply) => {
                let out = run_event(&mut vm, &kind, &payload).map(|response| Outcome { response, skip_waiting: skip_waiting_called() });
                let _ = reply.send(out);
            }
        }
    }
}

/// Whether `skipWaiting()` was called in this worker thread (carried back in each [`Outcome`]).
fn skip_waiting_called() -> bool {
    STATE.with(|s| s.borrow().as_ref().is_some_and(|s| s.skip_waiting))
}

/// AETHERWORKERS (SR123): `Headers` / `Request` / `Response` / `fetch` over `__sw_url` / `__sw_fetch` /
/// `__sw_utf8_*` — shared by the service-worker realm (run before [`SCOPE`]) and a dedicated worker's realm
/// (`workers::dedicated`), both of which register those natives through [`install_shared`].
/// `caches` on the service-worker global: the page prelude's getter over this realm's `__cache` (a secure
/// context answers the CacheStorage; an insecure one reads `undefined`, the `[SecureContext]` rule).
const CACHES: &str = r#"
(function () {
    var cs = __cache_shapes();
    Object.defineProperty(globalThis, 'caches', {
        get: function () { return __cache('secure', '{}') === true ? cs : undefined; },
        configurable: true, enumerable: true,
    });
})();
"#;

pub(crate) const FETCH: &str = r#"
(function () {
    var G = globalThis;
    var expose = function (name, v) { Object.defineProperty(G, name, { value: v, writable: true, configurable: true, enumerable: false }); };
    // ---- Headers / Request / Response / fetch (a synchronous exchange per call)
    var norm = function (k) { return String(k).toLowerCase(); };
    class Headers {
        constructor(init) {
            Object.defineProperty(this, '__l', { value: [], writable: true });
            var self = this;
            if (init instanceof Headers) { init.__l.forEach(function (p) { self.append(p[0], p[1]); }); }
            else if (Array.isArray(init)) { init.forEach(function (p) { self.append(p[0], p[1]); }); }
            else if (init && typeof init === 'object') { Object.keys(init).forEach(function (k) { self.append(k, init[k]); }); }
        }
        append(k, v) { this.__l.push([norm(k), String(v)]); }
        set(k, v) { k = norm(k); this.__l = this.__l.filter(function (p) { return p[0] !== k; }); this.__l.push([k, String(v)]); }
        get(k) {
            k = norm(k);
            var vs = this.__l.filter(function (p) { return p[0] === k; }).map(function (p) { return p[1]; });
            return vs.length ? vs.join(', ') : null;
        }
        has(k) { k = norm(k); return this.__l.some(function (p) { return p[0] === k; }); }
        delete(k) { k = norm(k); this.__l = this.__l.filter(function (p) { return p[0] !== k; }); }
        forEach(cb, t) { var self = this; this.__l.slice().forEach(function (p) { cb.call(t, p[1], p[0], self); }); }
        entries() { return this.__l.map(function (p) { return [p[0], p[1]]; })[Symbol.iterator](); }
        keys() { return this.__l.map(function (p) { return p[0]; })[Symbol.iterator](); }
        values() { return this.__l.map(function (p) { return p[1]; })[Symbol.iterator](); }
        [Symbol.iterator]() { return this.entries(); }
    }
    var toBytes = function (body) {
        if (body === undefined || body === null) { return null; }
        if (body instanceof ArrayBuffer) { return body.slice(0); }
        if (ArrayBuffer.isView(body)) { return body.buffer.slice(body.byteOffset, body.byteOffset + body.byteLength); }
        if (typeof Blob === 'function' && body instanceof Blob && typeof __blob_bytes === 'function') { return new Uint8Array(__blob_bytes(body)).buffer; }
        return __sw_utf8_encode(String(body));
    };
    var bodyMixin = function (C) {
        C.prototype.arrayBuffer = function () {
            if (this.__used) { return Promise.reject(new TypeError('Body has already been consumed.')); }
            this.__used = true;
            // An opaque response's body is not script's to read (Fetch §2.2.6 opaque filtered response).
            if (this.__type === 'opaque') { return Promise.resolve(new ArrayBuffer(0)); }
            return Promise.resolve(this.__b === null ? new ArrayBuffer(0) : this.__b.slice(0));
        };
        C.prototype.text = function () { return this.arrayBuffer().then(function (b) { return __sw_utf8_decode(b); }); };
        C.prototype.json = function () { return this.text().then(function (t) { return JSON.parse(t); }); };
        Object.defineProperty(C.prototype, 'bodyUsed', { get: function () { return !!this.__used; }, configurable: true });
    };
    var hidden = function (o, k, v) { Object.defineProperty(o, k, { value: v, writable: true, configurable: true, enumerable: false }); };
    class Request {
        constructor(input, init) {
            init = init || {};
            var src = input instanceof Request ? input : null;
            var url = src ? src.url : __sw_url(String(input), null);
            if (url === null) { throw new TypeError("Failed to construct 'Request': Invalid URL: " + input); }
            hidden(this, '__url', url);
            hidden(this, '__method', String(init.method || (src ? src.method : 'GET')).toUpperCase());
            hidden(this, '__headers', new Headers(init.headers || (src ? src.headers : undefined)));
            hidden(this, '__mode', init.mode || (src ? src.mode : 'cors'));
            hidden(this, '__dest', init.__destination !== undefined ? String(init.__destination) : (src ? src.__dest : ''));
            hidden(this, '__b', init.body !== undefined ? toBytes(init.body) : (src ? src.__b : null));
            hidden(this, '__used', false);
        }
        get url() { return this.__url; }
        get method() { return this.__method; }
        get headers() { return this.__headers; }
        get mode() { return this.__mode; }
        get destination() { return this.__dest; }
        clone() { return new Request(this); }
    }
    bodyMixin(Request);
    class Response {
        constructor(body, init) {
            init = init || {};
            var st = init.status === undefined ? 200 : Number(init.status);
            if (!(st >= 200 && st <= 599)) { throw new RangeError("Failed to construct 'Response': The status provided (" + st + ') is outside the range [200, 599].'); }
            hidden(this, '__status', st);
            hidden(this, '__statusText', init.statusText === undefined ? '' : String(init.statusText));
            hidden(this, '__headers', new Headers(init.headers));
            hidden(this, '__b', toBytes(body));
            hidden(this, '__url', '');
            hidden(this, '__type', 'default');
            hidden(this, '__used', false);
            if (typeof body === 'string' && !this.__headers.has('content-type')) { this.__headers.set('content-type', 'text/plain;charset=UTF-8'); }
        }
        get status() { return this.__status; }
        get statusText() { return this.__statusText; }
        get ok() { return this.__status >= 200 && this.__status <= 299; }
        get headers() { return this.__headers; }
        get url() { return this.__url; }
        get type() { return this.__type; }
        get redirected() { return false; }
        clone() {
            var r = new Response(this.__b, { status: this.__status, statusText: this.__statusText, headers: this.__headers });
            r.__url = this.__url; r.__type = this.__type;
            return r;
        }
        static error() { var r = new Response(null, { status: 200 }); r.__status = 0; r.__type = 'error'; return r; }
        static redirect(url, status) { return new Response(null, { status: status || 302, headers: { location: __sw_url(String(url), null) } }); }
        static json(data, init) {
            var h = new Headers((init || {}).headers);
            if (!h.has('content-type')) { h.set('content-type', 'application/json'); }
            return new Response(JSON.stringify(data), { status: (init || {}).status, statusText: (init || {}).statusText, headers: h });
        }
    }
    bodyMixin(Response);
    expose('Headers', Headers);
    expose('Request', Request);
    expose('Response', Response);
    expose('fetch', function fetch(input, init) {
        return new Promise(function (resolve, reject) {
            try {
                var q = new Request(input, init);
                var flat = [];
                q.headers.forEach(function (v, k) { flat.push(k, v); });
                var a = __sw_fetch(q.method, q.url, flat, q.__b === null ? undefined : q.__b, q.mode);
                var hl = [];
                for (var i = 0; i + 1 < a[3].length; i += 2) { hl.push([a[3][i], a[3][i + 1]]); }
                var st = a[0] >= 200 && a[0] <= 599 ? a[0] : 200;
                var nullBody = [101, 103, 204, 205, 304].indexOf(st) >= 0;
                var r = new Response(nullBody ? null : a[4], { status: st, statusText: a[1], headers: hl });
                r.__status = a[0]; r.__url = a[2]; r.__type = a[5] || (a[0] === 0 ? 'opaque' : 'basic');
                resolve(r);
            } catch (e) { reject(e instanceof TypeError ? e : new TypeError(String(e && e.message || e))); }
        });
    });
})();
"#;

/// The ServiceWorkerGlobalScope members over the natives above (after [`FETCH`]).
const SCOPE: &str = r#"
(function () {
    var G = globalThis;
    var hide = function (name, v) { Object.defineProperty(G, name, { value: v, writable: true, configurable: true, enumerable: false }); };
    var expose = function (name, v) { Object.defineProperty(G, name, { value: v, writable: true, configurable: true, enumerable: false }); };
    expose('self', G);
    if (typeof DOMException !== 'function') {
        class DOMException extends Error {
            constructor(message, name) { super(message === undefined ? '' : String(message)); this.name = name === undefined ? 'Error' : String(name); }
        }
        expose('DOMException', DOMException);
    }
    if (typeof __ledger !== 'function') { hide('__ledger', function () {}); }

    // ---- events (the global is the only target; listeners in registration order, then the handler)
    var L = Object.create(null);
    expose('addEventListener', function (type, f) {
        if (f === null || f === undefined) { return; }
        type = String(type);
        var list = L[type] || (L[type] = []);
        if (list.indexOf(f) < 0) { list.push(f); }
    });
    expose('removeEventListener', function (type, f) {
        var list = L[String(type)];
        if (!list) { return; }
        var i = list.indexOf(f);
        if (i >= 0) { list.splice(i, 1); }
    });
    ['install', 'activate', 'fetch', 'message', 'messageerror'].forEach(function (n) {
        var v = null;
        Object.defineProperty(G, 'on' + n, { get: function () { return v; }, set: function (f) { v = typeof f === 'function' ? f : null; }, configurable: true, enumerable: true });
    });
    var fire = function (ev) {
        var list = (L[ev.type] || []).slice();
        var h = G['on' + ev.type];
        if (typeof h === 'function') { list.push(h); }
        for (var i = 0; i < list.length; i++) {
            var f = list[i];
            try {
                if (typeof f === 'function') { f.call(G, ev); }
                else if (f && typeof f.handleEvent === 'function') { f.handleEvent(ev); }
            } catch (e) {
                console.error('Uncaught ' + (e && e.name ? e.name + ': ' + e.message + '\n' : '') + (e && e.stack ? e.stack : e));
            }
            if (ev.__stopImmediate) { break; }
        }
    };

    var hidden = function (o, k, v) { Object.defineProperty(o, k, { value: v, writable: true, configurable: true, enumerable: false }); };

    expose('importScripts', function () {
        for (var i = 0; i < arguments.length; i++) {
            var u = __sw_url(String(arguments[i]), null);
            if (u === null) { throw new DOMException("Failed to execute 'importScripts': The URL '" + arguments[i] + "' is invalid.", 'SyntaxError'); }
            __sw_import(u);
        }
    });

    // ---- events of the lifecycle
    class ExtendableEvent {
        constructor(type) { hidden(this, '__p', []); hidden(this, '__active', true); this.type = String(type); }
        waitUntil(p) {
            if (!this.__active && this.__p.length === 0) {
                throw new DOMException("Failed to execute 'waitUntil' on 'ExtendableEvent': The event handler is already finished.", 'InvalidStateError');
            }
            this.__p.push(Promise.resolve(p));
        }
        stopImmediatePropagation() { hidden(this, '__stopImmediate', true); }
        stopPropagation() {}
        preventDefault() {}
    }
    class FetchEvent extends ExtendableEvent {
        constructor(type, init) {
            super(type);
            this.request = init.request;
            this.clientId = init.clientId || '';
            this.resultingClientId = init.resultingClientId || '';
            this.handled = Promise.resolve(undefined);
            hidden(this, '__r', null);
        }
        respondWith(p) {
            if (this.__r !== null) { throw new DOMException("Failed to execute 'respondWith' on 'FetchEvent': The event has already been responded to.", 'InvalidStateError'); }
            if (!this.__active) { throw new DOMException("Failed to execute 'respondWith' on 'FetchEvent': The event handler is already finished.", 'InvalidStateError'); }
            this.__r = Promise.resolve(p);
        }
    }
    class ExtendableMessageEvent extends ExtendableEvent {
        constructor(type, init) {
            super(type);
            this.data = init.data; this.origin = init.origin || ''; this.lastEventId = ''; this.source = init.source || null; this.ports = [];
        }
    }
    expose('ExtendableEvent', ExtendableEvent);
    expose('FetchEvent', FetchEvent);
    expose('ExtendableMessageEvent', ExtendableMessageEvent);

    // ---- clients, registration, skipWaiting
    // AETHERWORKERS3 (SR129): every client this engine tracks is a top-level document, so each is a WindowClient
    // (Service Workers §4.3): `visibilityState` 'visible' (a headless page is never hidden), `focused` false, and
    // `focus()` rejects with InvalidAccessError — the spec's answer when the worker has no transient user
    // activation to spend ("If this algorithm is not triggered by user activation, reject").
    var clientObj = function (c) {
        return {
            id: c.id, url: c.url, type: c.type || 'window', frameType: 'top-level',
            visibilityState: 'visible', focused: false,
            postMessage: function (msg) { __sw_client_post(c.id, __sc_ser(msg)); },
            focus: function () { return Promise.reject(new DOMException("Failed to execute 'focus' on 'WindowClient': Not allowed to focus a window.", 'InvalidAccessError')); },
        };
    };
    // ClientQueryOptions.type (Service Workers §4.4.2): 'window' by default; 'worker' / 'sharedworker' match no
    // tracked client (dedicated and shared workers are not service-worker clients here); 'all' matches every one.
    var CLIENT_TYPES = ['window', 'worker', 'sharedworker', 'all'];
    expose('clients', {
        get: function (id) {
            return Promise.resolve(JSON.parse(__sw_clients(true)).filter(function (c) { return c.id === String(id); }).map(clientObj)[0]);
        },
        matchAll: function (o) {
            var all = !!(o && o.includeUncontrolled);
            var type = (o && o.type !== undefined) ? String(o.type) : 'window';
            if (CLIENT_TYPES.indexOf(type) < 0) {
                return Promise.reject(new TypeError("Failed to execute 'matchAll' on 'Clients': The provided value '" + type + "' is not a valid enum value of type ClientType."));
            }
            return Promise.resolve(JSON.parse(__sw_clients(all)).filter(function (c) { return type === 'all' || (c.type || 'window') === type; }).map(clientObj));
        },
        claim: function () { __sw_claim(); return Promise.resolve(undefined); },
        openWindow: function () { __ledger('clients.openWindow-unsupported'); return Promise.reject(new TypeError('clients.openWindow is not supported.')); },
    });
    var skipped = false;
    expose('skipWaiting', function () { skipped = true; __sw_skip_waiting(); return Promise.resolve(undefined); });
    hide('__sw_init', function (j) {
        var c = JSON.parse(j);
        expose('registration', { scope: c.scope, installing: null, waiting: null, active: null,
            update: function () { return Promise.resolve(undefined); },
            unregister: function () { __ledger('registration.unregister-from-worker'); return Promise.resolve(false); } });
        expose('serviceWorker', { scriptURL: c.scriptURL, state: 'parsed' });
        expose('location', { href: c.scriptURL, toString: function () { return c.scriptURL; } });
    });

    // ---- dispatch, as the thread asks
    var JOBS = new Map(), next = 0;
    var unhex = function (h) {
        var u8 = new Uint8Array(h.length >> 1);
        for (var i = 0; i < u8.length; i++) { u8[i] = parseInt(h.substr(i * 2, 2), 16); }
        return u8.buffer;
    };
    hide('__sw_dispatch', function (type, payload) {
        var id = ++next, p = JSON.parse(payload), ev;
        if (type === 'fetch') {
            var req = new Request(p.url, { method: p.method, headers: p.headers, body: p.body ? unhex(p.body) : undefined, mode: p.mode, __destination: p.destination || '' });
            ev = new FetchEvent('fetch', { request: req, clientId: p.clientId, resultingClientId: p.resultingClientId });
        } else if (type === 'message') {
            var data;
            try { data = __sc_de(p.data); } catch (e) { type = 'messageerror'; data = null; }
            ev = new ExtendableMessageEvent(type, { data: data, origin: p.origin, source: p.source ? clientObj(p.source) : null });
        } else {
            ev = new ExtendableEvent(type);
        }
        var job = { done: false, ok: true, resp: null, body: null, err: null, skip: false };
        JOBS.set(id, job);
        fire(ev);
        ev.__active = false;
        var finish = function () { job.skip = skipped; job.done = true; };
        var fail = function (e) { job.ok = false; job.err = String(e && e.message !== undefined ? e.message : e); job.done = true; };
        if (type === 'fetch') {
            if (ev.__r === null) { finish(); return id; }
            // §6.4 "handle fetch": a rejected respondWith, a non-Response or a network-error Response is a
            // NETWORK ERROR for the request (type 'error'), never the network fallback.
            var netError = function (e) {
                if (e !== undefined) { console.error('respondWith: ' + String(e && e.message !== undefined ? e.message : e)); }
                job.resp = { status: 0, statusText: '', headers: [], url: '', type: 'error' };
                job.body = null;
                finish();
            };
            ev.__r.then(function (r) {
                if (!(r instanceof Response)) { throw new TypeError('The FetchEvent resolved with a value that is not a Response.'); }
                if (r.type === 'error') { return 'error'; }
                if (r.__used) { throw new TypeError('The FetchEvent resolved with a Response whose body is already used.'); }
                r.__used = true;
                var hl = [];
                r.headers.forEach(function (v, k) { hl.push([k, v]); });
                job.resp = { status: r.status, statusText: r.statusText, headers: hl, url: r.url || p.url, type: r.type };
                job.body = r.__b === null ? new ArrayBuffer(0) : r.__b.slice(0);
            }).then(function (v) { if (v === 'error') { netError(); } else { finish(); } }, netError);
            return id;
        }
        Promise.all(ev.__p).then(finish, fail);
        return id;
    });
    hide('__sw_status', function (id) {
        var j = JOBS.get(id);
        if (!j) { return 'null'; }
        return JSON.stringify({ done: j.done, ok: j.ok, resp: j.resp, err: j.err, skip: j.skip });
    });
    hide('__sw_body', function (id) {
        var j = JOBS.get(id);
        JOBS.delete(id);
        return j && j.body ? j.body : new ArrayBuffer(0);
    });
})();
"#;
