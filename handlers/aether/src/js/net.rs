//! AETHERNET (LEDGER SR70): the page's networking task source and the natives under `WebSocket`,
//! `EventSource`, streaming `fetch`, `XMLHttpRequest` and Web Storage's `storage` event.
//!
//! CHARTER: Aether owns the browser (CODEX §2); the transport is gneiss_pal::api::http (HTTPCORE SR51) —
//! http_core + tls_core for HTTP(S), its `ws` module for RFC 6455.
//!
//! Shape: every network operation runs on its own worker thread (`api::fetch`, `api::websockets`,
//! `api::eventsource`) and reports through a [`Poster`] into this thread's queue. The page's event loop
//! drains the queue in [`poll`] — from `Engine::tick`, the boot drain and `event_loop::settle` — and each
//! message becomes a task: the JS-side dispatcher (`__net_dispatch`, defined by [`PRELUDE`]) is invoked,
//! followed by a microtask checkpoint. So a body chunk, a socket message or a server-sent event reaches
//! script on the event loop, in arrival order, never re-entrantly inside another script.
//!
//! A new page (a new `Engine`) bumps the queue's generation and cancels every operation of the previous
//! one: a late message from a dead page is dropped by generation, and its worker sees its cancel flag.
//!
//! The WHATWG object shapes (classes, attributes, event handler IDL attributes, readyState machines) are
//! JS preludes beside each API (`api::fetch::PRELUDE`, …), over the natives here — the split `api::fetch`
//! and `api::platform` already use.

use super::idl::{self, arg, num, s};
use js_core::builtins::typedarray::new_array_buffer;
use js_core::vm::*;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};

/// Most network operations one page may have in flight (a runaway page cannot exhaust threads).
pub const MAX_OPS: usize = 64;
/// Most queued network messages one event-loop turn turns into tasks (the rest wait for the next turn).
pub const MAX_MSGS_PER_TURN: usize = 512;

/// One argument of a message, converted to a JS value when the task runs.
pub enum NetArg {
    Num(f64),
    Str(String),
    Bool(bool),
    Bytes(Vec<u8>),
    List(Vec<String>),
    Null,
}

/// One message from a worker: operation `id`, a kind the JS dispatcher switches on, its arguments.
/// `last` ends the operation (its registry entry is dropped once the task has run).
pub struct NetMsg {
    pub id: u32,
    pub kind: &'static str,
    pub args: Vec<NetArg>,
    pub last: bool,
}

/// A command the page sends an open connection.
pub enum Cmd {
    WsText(String),
    WsBinary(Vec<u8>),
    WsClose(Option<u16>, String),
}

/// Messages carry the generation of the page that started their operation (`ANY_EPOCH` = a broadcast).
const ANY_EPOCH: u64 = u64::MAX;

/// What a worker thread holds: where to post, and the flag that says the page let go of it.
#[derive(Clone)]
pub struct Poster {
    id: u32,
    epoch: u64,
    tx: Sender<(u64, NetMsg)>,
    cancel: Arc<AtomicBool>,
}

impl Poster {
    pub fn id(&self) -> u32 {
        self.id
    }
    /// True once the page aborted / closed the operation or went away.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
    /// Posts a message; `false` when the page is gone (the worker should stop).
    pub fn post(&self, kind: &'static str, args: Vec<NetArg>) -> bool {
        if self.cancelled() {
            return false;
        }
        self.tx.send((self.epoch, NetMsg { id: self.id, kind, args, last: false })).is_ok()
    }
    /// Posts the operation's final message.
    pub fn finish(&self, kind: &'static str, args: Vec<NetArg>) {
        if !self.cancelled() {
            let _ = self.tx.send((self.epoch, NetMsg { id: self.id, kind, args, last: true }));
        }
    }
    /// Sleeps `ms` in short steps, returning early (false) when the operation is cancelled.
    pub fn sleep(&self, ms: u64) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_millis(ms);
        while std::time::Instant::now() < end {
            if self.cancelled() {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(5).min(end - std::time::Instant::now()));
        }
        !self.cancelled()
    }
}

struct Op {
    cancel: Arc<AtomicBool>,
    cmd: Option<Sender<Cmd>>,
}

struct NetState {
    epoch: u64,
    tx: Sender<(u64, NetMsg)>,
    rx: Receiver<(u64, NetMsg)>,
    ops: HashMap<u32, Op>,
    next_id: u32,
    /// This thread's entry in the process-wide document registry (the `storage` event's audience).
    doc: u64,
    /// The `__net_dispatch` function (host root), once the prelude defined it.
    dispatch_root: Option<usize>,
}

impl NetState {
    fn new() -> Self {
        let (tx, rx) = channel();
        let doc = DOC_SERIAL.fetch_add(1, Ordering::Relaxed) + 1;
        NetState { epoch: 1, tx, rx, ops: HashMap::new(), next_id: 0, doc, dispatch_root: None }
    }
}

impl Drop for NetState {
    fn drop(&mut self) {
        for op in self.ops.values() {
            op.cancel.store(true, Ordering::Relaxed);
        }
        if let Ok(mut docs) = docs().lock() {
            docs.retain(|d| d.serial != self.doc);
        }
    }
}

thread_local! {
    static NET: RefCell<NetState> = RefCell::new(NetState::new());
}

static DOC_SERIAL: AtomicU64 = AtomicU64::new(0);

/// A document of this process that can receive a `storage` event: its queue and its current URL.
struct DocEntry {
    serial: u64,
    tx: Sender<(u64, NetMsg)>,
    url: Arc<Mutex<String>>,
}

fn docs() -> &'static Mutex<Vec<DocEntry>> {
    static DOCS: OnceLock<Mutex<Vec<DocEntry>>> = OnceLock::new();
    DOCS.get_or_init(|| Mutex::new(Vec::new()))
}

/// The page's origin (`scheme://host[:port]`), or an opaque `null:<doc>` that matches no other document.
pub fn page_origin() -> String {
    // AETHERWORKERS2 (SR125): on a dedicated worker thread, the worker's origin (`storage::WorkerClient`).
    if let Some(o) = crate::storage::worker_client_origin() {
        return o;
    }
    let url = super::page_url();
    origin_of(&url).unwrap_or_else(|| format!("null:{}", NET.with(|n| n.borrow().doc)))
}

fn origin_of(url: &str) -> Option<String> {
    let u = gneiss_pal::api::http::Url::parse(url).ok()?;
    let o = u.origin();
    (o != "null").then_some(o)
}

/// A new page on this thread: cancels the previous page's operations, starts a new generation, and
/// (re)registers the document for `storage` events.
pub fn reset() {
    NET.with(|n| {
        let mut n = n.borrow_mut();
        for (_, op) in n.ops.drain() {
            op.cancel.store(true, Ordering::Relaxed);
        }
        n.epoch += 1;
        n.dispatch_root = None;
        while n.rx.try_recv().is_ok() {}
        let (serial, tx) = (n.doc, n.tx.clone());
        if let Ok(mut d) = docs().lock() {
            if !d.iter().any(|e| e.serial == serial) {
                d.push(DocEntry { serial, tx, url: Arc::new(Mutex::new(String::new())) });
            }
        }
    });
}

/// Keeps the registry's copy of this document's URL current (the `storage` event's `url` is the
/// *writer's*; the receiver filters by its own origin).
pub fn note_url(url: &str) {
    let serial = NET.with(|n| n.borrow().doc);
    if let Ok(d) = docs().lock() {
        if let Some(e) = d.iter().find(|e| e.serial == serial) {
            if let Ok(mut u) = e.url.lock() {
                *u = url.to_string();
            }
        }
    }
}

/// Registers a new operation; `None` when the page is at [`MAX_OPS`].
pub fn start_op(cmd: Option<Sender<Cmd>>) -> Option<Poster> {
    NET.with(|n| {
        let mut n = n.borrow_mut();
        if n.ops.len() >= MAX_OPS {
            crate::ledger::record_js("net-ops-cap-reached");
            return None;
        }
        n.next_id = n.next_id.wrapping_add(1).max(1);
        let id = n.next_id;
        let cancel = Arc::new(AtomicBool::new(false));
        n.ops.insert(id, Op { cancel: cancel.clone(), cmd });
        Some(Poster { id, epoch: n.epoch, tx: n.tx.clone(), cancel })
    })
}

/// The page let go of operation `id` (abort, close): its worker sees the flag; later messages drop.
pub fn cancel_op(id: u32) {
    NET.with(|n| {
        if let Some(op) = n.borrow_mut().ops.remove(&id) {
            op.cancel.store(true, Ordering::Relaxed);
        }
    });
}

/// Sends a command to an open connection; false when it is gone.
pub fn command(id: u32, c: Cmd) -> bool {
    NET.with(|n| n.borrow().ops.get(&id).and_then(|o| o.cmd.as_ref()).is_some_and(|tx| tx.send(c).is_ok()))
}

/// Whether network operations are in flight (the virtual-time drivers wait on real time for them).
pub fn active() -> bool {
    NET.with(|n| !n.borrow().ops.is_empty())
}

/// Posts a `storage` event to every OTHER document of the process (the receiver keeps it only when the
/// writer's origin is its own).
pub fn broadcast_storage(origin: &str, key: Option<&str>, old: Option<&str>, new: Option<&str>, url: &str) {
    let me = NET.with(|n| n.borrow().doc);
    let opt = |v: Option<&str>| v.map(|x| NetArg::Str(x.to_string())).unwrap_or(NetArg::Null);
    if let Ok(mut d) = docs().lock() {
        d.retain(|e| {
            if e.serial == me {
                return true;
            }
            let msg = NetMsg {
                id: 0,
                kind: "storage",
                args: vec![NetArg::Str(origin.to_string()), opt(key), opt(old), opt(new), NetArg::Str(url.to_string())],
                last: false,
            };
            e.tx.send((ANY_EPOCH, msg)).is_ok()
        });
    }
}

fn to_value(vm: &mut Vm, a: NetArg) -> Value {
    match a {
        NetArg::Num(n) => num(n),
        NetArg::Str(t) => s(&t),
        NetArg::Bool(b) => Value::Bool(b),
        NetArg::Null => Value::Null,
        NetArg::Bytes(b) => array_buffer(vm, &b).map(Value::Object).unwrap_or(Value::Null),
        NetArg::List(l) => {
            let vals: Vec<Value> = l.iter().map(|x| s(x)).collect();
            Value::Object(vm.new_array(vals))
        }
    }
}

/// A new `ArrayBuffer` holding `bytes`.
pub fn array_buffer(vm: &mut Vm, bytes: &[u8]) -> JsResult<Obj> {
    let b = new_array_buffer(vm, bytes.len())?;
    if let Kind::ArrayBuffer(d) = &mut vm.heap.get_mut(b).kind {
        d.data[..bytes.len()].copy_from_slice(bytes);
    }
    Ok(b)
}

/// The bytes of a `BufferSource` (ArrayBuffer, a typed array or a DataView, honouring offset/length).
pub fn buffer_bytes(vm: &Vm, v: &Value) -> Option<Vec<u8>> {
    let o = v.as_object()?;
    let buf_of = |b: Obj| -> Option<Vec<u8>> {
        match &vm.heap.get(b).kind {
            Kind::ArrayBuffer(d) if !d.detached => Some(d.data.clone()),
            Kind::ArrayBuffer(_) => Some(Vec::new()),
            _ => None,
        }
    };
    match &vm.heap.get(o).kind {
        Kind::ArrayBuffer(_) => buf_of(o),
        Kind::TypedArray(t) => {
            let all = buf_of(t.buffer)?;
            let start = t.byte_offset.min(all.len());
            let end = match t.length {
                Some(n) => (start + n * t.kind.size()).min(all.len()),
                None => all.len(),
            };
            Some(all[start..end].to_vec())
        }
        Kind::DataView(d) => {
            let all = buf_of(d.buffer)?;
            let start = d.byte_offset.min(all.len());
            let end = match d.byte_length {
                Some(n) => (start + n).min(all.len()),
                None => all.len(),
            };
            Some(all[start..end].to_vec())
        }
        _ => None,
    }
}

/// Runs the queued network tasks (at most [`MAX_MSGS_PER_TURN`]). Returns how many ran.
pub fn poll(vm: &mut Vm) -> usize {
    if super::engine_poisoned() {
        return 0;
    }
    let mut ran = 0;
    while ran < MAX_MSGS_PER_TURN {
        let (msg, f) = match NET.with(|n| {
            let mut n = n.borrow_mut();
            loop {
                let (epoch, msg) = n.rx.try_recv().ok()?;
                if epoch != n.epoch && epoch != ANY_EPOCH {
                    continue; // a previous page's late message
                }
                if msg.id != 0 && !n.ops.contains_key(&msg.id) {
                    continue; // aborted / closed by the page
                }
                if msg.last {
                    n.ops.remove(&msg.id);
                }
                return Some((msg, n.dispatch_root));
            }
        }) {
            Some(x) => x,
            None => break,
        };
        ran += 1;
        let Some(root) = f else { continue };
        let f = idl::rooted(vm, root);
        if msg.kind == "storage" {
            // Only a document of the writer's origin receives it.
            let origin = match msg.args.first() {
                Some(NetArg::Str(o)) => o.clone(),
                _ => String::new(),
            };
            if origin != page_origin() {
                continue;
            }
        }
        let mut args = vec![num(msg.id as f64), s(msg.kind), Value::Bool(msg.last)];
        for a in msg.args {
            let v = to_value(vm, a);
            args.push(v);
        }
        super::invoke(vm, "net-task", &f, &Value::Undefined, &args);
        super::checkpoint(vm);
        if super::engine_poisoned() {
            break;
        }
    }
    ran
}

// ------------------------------------------------------------------------------------------------ natives

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

fn list_arg(vm: &mut Vm, v: &Value) -> JsResult<Vec<String>> {
    let mut out = Vec::new();
    if let Value::Object(o) = v {
        let len = vm.get(*o, &PropertyKey::from_str("length"))?;
        let n = vm.to_number(&len)? as usize;
        for i in 0..n.min(4096) {
            let e = vm.get(*o, &PropertyKey::from_str(&i.to_string()))?;
            out.push(vm.to_string(&e)?.to_rust());
        }
    }
    Ok(out)
}

/// `__net_fire(target, iface, type, init, cancelable)`: fires a trusted event of interface `iface` (its members
/// read from `init`) at an EventTarget; answers whether it was not canceled. `cancelable` (AETHERWORKERS2,
/// SR125; absent = false) makes the event cancelable — the Worker's ErrorEvent (HTML §10.2.5).
fn net_fire(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let t = arg(vm, ctx, 0);
    let Some(target) = super::events::target_of(vm, &t) else { return vm.throw_type("__net_fire: not an EventTarget") };
    let iface = str_arg(vm, ctx, 1)?;
    let ty = str_arg(vm, ctx, 2)?;
    let init = arg(vm, ctx, 3).as_object();
    let cancelable = arg(vm, ctx, 4);
    let cancelable = vm.to_boolean(&cancelable);
    let e = super::events::new_event(vm, &iface, &ty, false, cancelable, init);
    let r = idl::root(vm, Value::Object(e));
    let (not_canceled, _) = super::events::dispatch(vm, e, target);
    idl::unroot(vm, r);
    Ok(Value::Bool(not_canceled))
}

/// The body argument of a request: a string (UTF-8), a BufferSource, or nothing.
fn body_arg(vm: &mut Vm, v: &Value) -> JsResult<Vec<u8>> {
    if v.is_undefined() || v.is_null() {
        return Ok(Vec::new());
    }
    if let Some(b) = buffer_bytes(vm, v) {
        return Ok(b);
    }
    Ok(vm.to_string(v)?.to_rust().into_bytes())
}

fn request_arg(vm: &mut Vm, ctx: &CallCtx) -> JsResult<crate::api::fetch::HttpReq> {
    let method = str_arg(vm, ctx, 0)?;
    let url = str_arg(vm, ctx, 1)?;
    let hv = arg(vm, ctx, 2);
    let flat = list_arg(vm, &hv)?;
    let headers = flat.chunks(2).filter(|c| c.len() == 2).map(|c| (c[0].clone(), c[1].clone())).collect();
    let bv = arg(vm, ctx, 3);
    let body = body_arg(vm, &bv)?;
    let redirect = {
        let r = arg(vm, ctx, 4);
        if r.is_undefined() { "follow".to_string() } else { vm.to_string(&r)?.to_rust() }
    };
    let timeout_ms = {
        let t = arg(vm, ctx, 5);
        if t.is_undefined() { 0.0 } else { vm.to_number(&t)? }
    };
    Ok(crate::api::fetch::HttpReq { method, url, headers, body, redirect, timeout_ms: timeout_ms.max(0.0) as u64 })
}

/// `__net_http(method, url, [k, v, …], body, redirect, timeoutMs)` → operation id (0 = refused). The
/// worker posts `head(status, statusText, url, [k, v, …])`, `data(ArrayBuffer)`…, then `end` or `error`.
fn net_http(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let req = request_arg(vm, ctx)?;
    if !(req.url.starts_with("http://") || req.url.starts_with("https://")) {
        crate::ledger::record_js("fetch-bad-url");
        return Ok(num(0.0));
    }
    let Some(p) = start_op(None) else { return Ok(num(0.0)) };
    let id = p.id();
    crate::api::fetch::spawn_http(req, p);
    Ok(num(id as f64))
}

/// `__net_http_sync(…)` (synchronous XHR): the whole exchange, joined on this thread —
/// `{status, statusText, url, headers, body}` or `{error}`.
fn net_http_sync(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let req = request_arg(vm, ctx)?;
    let o = vm.new_plain_object();
    match crate::api::fetch::http_sync(req) {
        Ok(r) => {
            vm.create_data_property(o, PropertyKey::from_str("status"), num(r.status as f64))?;
            vm.create_data_property(o, PropertyKey::from_str("statusText"), s(&r.status_text))?;
            vm.create_data_property(o, PropertyKey::from_str("url"), s(&r.url))?;
            let hv = to_value(vm, NetArg::List(r.headers));
            vm.create_data_property(o, PropertyKey::from_str("headers"), hv)?;
            let b = array_buffer(vm, &r.body)?;
            vm.create_data_property(o, PropertyKey::from_str("body"), Value::Object(b))?;
        }
        Err(e) => {
            vm.create_data_property(o, PropertyKey::from_str("error"), s(&e))?;
        }
    }
    Ok(Value::Object(o))
}

/// `__net_cancel(id)`: abort a fetch / XHR, or drop an EventSource.
fn net_cancel(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let v = arg(vm, ctx, 0);
    let id = vm.to_number(&v)?;
    if id.is_finite() && id > 0.0 {
        cancel_op(id as u32);
    }
    Ok(Value::Undefined)
}

/// `__net_ws_open(url, [protocols])` → id (0 = refused). Posts `open(protocol, extensions)`,
/// `text(s)` / `binary(ArrayBuffer)`, `sent(n)`, `error`, and finally `close(wasClean, code, reason)`.
fn net_ws_open(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let url = str_arg(vm, ctx, 0)?;
    let pv = arg(vm, ctx, 1);
    let protocols = list_arg(vm, &pv)?;
    let (tx, rx) = channel();
    let Some(p) = start_op(Some(tx)) else { return Ok(num(0.0)) };
    let id = p.id();
    let origin = origin_of(&super::page_url()).unwrap_or_else(|| "null".into());
    crate::api::websockets::spawn(url, protocols, origin, p, rx);
    Ok(num(id as f64))
}

/// `__net_ws_cookies(url, [set-cookie, …])`: the handshake response's cookies, stored when `open` runs.
fn net_ws_cookies(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let url = str_arg(vm, ctx, 0)?;
    let lv = arg(vm, ctx, 1);
    let list = list_arg(vm, &lv)?;
    let http_form = url.replacen("ws", "http", 1);
    if let Ok(u) = gneiss_pal::api::http::Url::parse(&http_form) {
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        if let Ok(mut jar) = crate::net::cookie_jar().lock() {
            for c in &list {
                jar.set_cookie(&u, c, now, true);
            }
        }
    }
    Ok(Value::Undefined)
}

/// `__net_ws_send(id, data)`: a string goes as a text message, a BufferSource as binary. Answers the
/// byte count queued (what `bufferedAmount` grows by).
fn net_ws_send(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let idv = arg(vm, ctx, 0);
    let id = vm.to_number(&idv)? as u32;
    let d = arg(vm, ctx, 1);
    let (cmd, n) = match buffer_bytes(vm, &d) {
        Some(b) => {
            let n = b.len();
            (Cmd::WsBinary(b), n)
        }
        None => {
            let t = vm.to_string(&d)?.to_rust();
            let n = t.len();
            (Cmd::WsText(t), n)
        }
    };
    command(id, cmd);
    Ok(num(n as f64))
}

/// `__net_ws_close(id, code | undefined, reason)`: start the closing handshake.
fn net_ws_close(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let idv = arg(vm, ctx, 0);
    let id = vm.to_number(&idv)? as u32;
    let cv = arg(vm, ctx, 1);
    let code = if cv.is_undefined() || cv.is_null() { None } else { Some(vm.to_number(&cv)? as u16) };
    let reason = str_arg(vm, ctx, 2)?;
    command(id, Cmd::WsClose(code, reason));
    Ok(Value::Undefined)
}

/// `__net_es_open(url, withCredentials)` → id. Posts `open`, `message(type, data, lastEventId, origin)`,
/// `error(reconnecting)` — and `error(false)` as its last message when the connection fails for good.
fn net_es_open(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let url = str_arg(vm, ctx, 0)?;
    let Some(p) = start_op(None) else { return Ok(num(0.0)) };
    let id = p.id();
    crate::api::eventsource::spawn(url, p);
    Ok(num(id as f64))
}

/// `__storage(area, op, a, b)` — Web Storage over `storage::` (`local` per origin, process-wide;
/// `session` per page thread). `set`/`remove`/`clear` that change the local area post the `storage` event
/// to the origin's other documents.
fn storage_op(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let area = str_arg(vm, ctx, 0)?;
    let op = str_arg(vm, ctx, 1)?;
    // AETHERDEBT SR93 (Storage Partitioning): the area is keyed by the storage key — the calling
    // context's origin AND the top-level site (`storage::storage_key`), so a document embedded under
    // another site does not share an area with the same origin at top level.
    let caller = super::window::frames::incumbent_origin(vm).unwrap_or_else(page_origin);
    let origin = crate::storage::storage_key(&super::page_url(), &caller);
    if op == "id" {
        return Ok(s(&origin));
    }
    let local = area == "local";
    let a = arg(vm, ctx, 2);
    let b = arg(vm, ctx, 3);
    let opt = |v: Option<String>| v.map(|x| s(&x)).unwrap_or(Value::Null);
    let on = |f: &mut dyn FnMut(&mut crate::storage::Area)| crate::storage::with_area(local, &origin, f);
    match op.as_str() {
        "length" => {
            let mut n = 0;
            on(&mut |ar| n = ar.len());
            Ok(num(n as f64))
        }
        "key" => {
            let i = vm.to_number(&a)?;
            let i = if i.is_finite() && i >= 0.0 { i as usize } else { usize::MAX };
            let mut k = None;
            on(&mut |ar| k = ar.key(i).map(str::to_string));
            Ok(opt(k))
        }
        "keys" => {
            let mut keys = Vec::new();
            on(&mut |ar| keys = ar.keys());
            Ok(to_value(vm, NetArg::List(keys)))
        }
        "get" => {
            let k = vm.to_string(&a)?.to_rust();
            let mut v = None;
            on(&mut |ar| v = ar.get(&k).map(str::to_string));
            Ok(opt(v))
        }
        "set" => {
            let k = vm.to_string(&a)?.to_rust();
            let v = vm.to_string(&b)?.to_rust();
            let mut r = Ok(None);
            on(&mut |ar| r = ar.set(&k, &v));
            match r {
                Err(()) => idl::throw_dom(
                    vm,
                    "QuotaExceededError",
                    &format!("Failed to execute 'setItem' on 'Storage': Setting the value of '{}' exceeded the quota.", super::clip(&k, 64)),
                ),
                Ok(Some(old)) if old == v => Ok(Value::Undefined),
                Ok(old) => {
                    if local {
                        broadcast_storage(&origin, Some(&k), old.as_deref(), Some(&v), &super::page_url());
                    }
                    Ok(Value::Undefined)
                }
            }
        }
        "remove" => {
            let k = vm.to_string(&a)?.to_rust();
            let mut old = None;
            on(&mut |ar| old = ar.remove(&k));
            if let (true, Some(old)) = (local, old) {
                broadcast_storage(&origin, Some(&k), Some(&old), None, &super::page_url());
            }
            Ok(Value::Undefined)
        }
        "clear" => {
            let mut had = false;
            on(&mut |ar| had = ar.clear());
            if local && had {
                broadcast_storage(&origin, None, None, None, &super::page_url());
            }
            Ok(Value::Undefined)
        }
        _ => vm.throw_type("__storage: unknown operation"),
    }
}

/// Installs the natives, the dispatcher, and the API preludes (fetch + streams, WebSocket, EventSource,
/// XMLHttpRequest, Web Storage). Runs after `window::install`, so it supersedes the platform prelude's
/// whole-body `fetch`, its synchronous `XMLHttpRequest`, its in-page `localStorage` and its
/// listener-array `AbortSignal`.
pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    for (name, len, f) in [
        ("__net_fire", 5, net_fire as NativeFn),
        ("__net_http", 6, net_http),
        ("__net_http_sync", 6, net_http_sync),
        ("__net_cancel", 1, net_cancel),
        ("__net_ws_open", 2, net_ws_open),
        ("__net_ws_send", 2, net_ws_send),
        ("__net_ws_close", 3, net_ws_close),
        ("__net_ws_cookies", 2, net_ws_cookies),
        ("__net_es_open", 2, net_es_open),
        ("__storage", 4, storage_op),
    ] {
        let fo = vm.make_native(name, len, f, false);
        vm.heap.get_mut(g).props.insert(PropertyKey::from_str(name), Prop::data(Value::Object(fo), WC));
    }
    for (label, src) in [
        ("net", PRELUDE),
        ("fetch", crate::api::fetch::PRELUDE),
        ("websocket", crate::api::websockets::PRELUDE),
        ("eventsource", crate::api::eventsource::PRELUDE),
        ("xhr", crate::api::xhr::PRELUDE),
        ("storage", crate::storage::PRELUDE),
    ] {
        if let Err(e) = vm.run_script_str(src) {
            let m = vm.error_string(&e);
            crate::ledger::record_js(&format!("{label}-prelude-failed:{}", super::clip(&m, 64)));
            eprintln!("[aether] {label} prelude failed: {m}");
        }
    }
    let d = vm.get(g, &PropertyKey::from_str("__net_dispatch")).unwrap_or(Value::Undefined);
    if vm.is_callable(&d) {
        let r = idl::root(vm, d);
        NET.with(|n| n.borrow_mut().dispatch_root = Some(r));
    }
    note_url(&super::page_url());
}

/// The dispatcher: operation id → the object that started it (a WebSocket, an EventSource, a fetch's
/// response stream, an XMLHttpRequest), whose `__net(kind, last, …args)` method runs the task.
const PRELUDE: &str = r#"
(function () {
    var reg = new Map();
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    hide('__net_register', function (id, obj) { reg.set(id, obj); });
    hide('__net_forget', function (id) { reg.delete(id); });
    hide('__net_dispatch', function (id, kind, last) {
        var args = Array.prototype.slice.call(arguments, 3);
        if (id === 0) {
            if (kind === 'storage' && typeof __storage_event === 'function') { __storage_event.apply(null, args); }
            return;
        }
        var o = reg.get(id);
        if (!o) { return; }
        if (last) { reg.delete(id); }
        o.__net(kind, args);
    });
    // Event handler IDL attributes (HTML §8.1.8.1) for the network interfaces: setting one registers an
    // internal listener once (at the position of the first set), which calls the current handler value.
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
                            // EventHandler is a callback *function* ([LegacyTreatNonObjectAsNull]): an object
                            // is stored but never invoked (no handleEvent lookup).
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
