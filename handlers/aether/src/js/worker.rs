//! AETHERWORKERS (LEDGER SR123): the page's `Worker` (HTML §10.2.6) over `crate::workers::dedicated`.
//!
//! CHARTER: Aether — binding (the page-facing half; the engine is `workers::dedicated`).
//!
//! `new Worker(url, { type, name })` parses `url` against the document's URL (a failure throws "SyntaxError"),
//! then decides and fetches on the page thread, so the request is the PAGE'S: the worker URL must be same
//! origin with the document or a `data:` URL (HTML §10.2.4 "fetch a classic/module worker script": request mode
//! `same-origin`; a `data:` worker gets an opaque origin), and `security::subresource_url(url, "worker")`
//! applies mixed content and CSP `worker-src` (falling back to `child-src`, `script-src`, `default-src`,
//! `security/csp.rs` fallback_list). A refusal or a failed fetch is not an exception: the Worker object
//! receives a simple `error` event as a task (`worker-error` through the document's inbox), as the spec's
//! "fetch failure" path says. The fetched script goes to the worker thread with the page's storage key and a
//! navigator snapshot.
//!
//! `worker.postMessage(message, transfer)` serializes through `js::ports`' `__msg_pack` (the
//! `js::broadcast` serializer with ArrayBuffer and MessagePort transfer) and sends one `message` task to the
//! worker; the worker's `postMessage` arrives as a `worker-message` inbox task here, which fires a trusted
//! MessageEvent (`data`, `origin` "", `ports`) — or `messageerror` when deserialization fails. An uncaught
//! worker error arrives as `worker-error` and fires an ErrorEvent (`message`, `filename`, `lineno`, `colno`).
//! `terminate()` stops the worker (`dedicated::terminate`) and drops any of its messages still queued here.

use super::idl::{self, arg, num};
use js_core::vm::*;
use serde_json::json;

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    if v.is_nullish() {
        return Ok(String::new());
    }
    Ok(vm.to_string(&v)?.to_rust())
}

fn id_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<u64> {
    let v = arg(vm, ctx, i);
    let n = vm.to_number(&v)?;
    Ok(if n.is_finite() && n >= 0.0 { n as u64 } else { 0 })
}

/// Queues the simple `error` event a refused or failed worker script fetch fires at the Worker.
fn fail(doc: u64, id: u64, rule: &str, url: &str, why: &str) {
    crate::ledger::record_js(&format!("worker-{rule}:{}", super::clip(url, 48)));
    super::broadcast::post(doc, "worker-error", json!({ "id": id, "simple": true, "message": why }).to_string());
}

/// `__worker(op, …)`: `spawn(url, type, name, navigatorJson)` → id; `post(id, data, ports)`; `terminate(id)`.
fn worker_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let op = str_arg(vm, ctx, 0)?;
    match op.as_str() {
        "spawn" => {
            let input = str_arg(vm, ctx, 1)?;
            let kind = if str_arg(vm, ctx, 2)? == "module" { crate::workers::dedicated::Kind::Module } else { crate::workers::dedicated::Kind::Classic };
            let name = str_arg(vm, ctx, 3)?;
            let navigator = str_arg(vm, ctx, 4)?;
            let base = super::page_url();
            let Ok(url) = html_core::url::Url::parse(&base).and_then(|b| b.join(&input)).map(|u| u.to_string()) else {
                return idl::throw_dom(vm, "SyntaxError", &format!("Failed to construct 'Worker': The URL '{input}' is invalid."));
            };
            let doc = super::broadcast::doc();
            let id = crate::workers::dedicated::next_id();
            use crate::security::origin::Origin;
            let me = Origin::of(&base);
            if !url.starts_with("data:") && !me.same_origin(&Origin::of(&url)) {
                let msg = format!("Failed to construct 'Worker': Script at '{url}' cannot be accessed from origin '{}'.", me.serialize());
                crate::security::refuse("worker-same-origin", &url, &msg);
                fail(doc, id, "refused", &url, &msg);
                return Ok(num(id as f64));
            }
            let Some(url) = crate::security::subresource_url(&url, "worker") else {
                fail(doc, id, "refused", &url, &format!("Refused to create a worker from '{url}' because it violates the document's Content Security Policy (worker-src)."));
                return Ok(num(id as f64));
            };
            let source = match super::host::fetch_script(&url, kind == crate::workers::dedicated::Kind::Module, None, "") {
                Ok(t) => t,
                Err(e) => {
                    fail(doc, id, "script-fetch-failed", &url, &format!("Failed to load the worker script '{url}': {e}"));
                    return Ok(num(id as f64));
                }
            };
            let sp = crate::workers::dedicated::Spawn {
                id,
                doc,
                url: url.clone(),
                kind,
                name,
                source,
                storage_key: crate::storage::page_storage_key().unwrap_or_default(),
                navigator,
            };
            if let Err(e) = crate::workers::dedicated::spawn(sp) {
                fail(doc, id, "thread-failed", &url, &format!("Failed to start the worker: {e}"));
            }
            Ok(num(id as f64))
        }
        "post" => {
            let id = id_arg(vm, ctx, 1)?;
            let data = str_arg(vm, ctx, 2)?;
            let ports = str_arg(vm, ctx, 3)?;
            crate::workers::dedicated::post(id, data, ports);
            Ok(Value::Undefined)
        }
        "terminate" => {
            let id = id_arg(vm, ctx, 1)?;
            crate::workers::dedicated::terminate(id);
            Ok(Value::Undefined)
        }
        _ => vm.throw_type("__worker: unknown operation"),
    }
}

/// Installs `Worker` (after `js::ports`, whose transfer helpers it uses).
pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__worker", 5, worker_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__worker"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("worker-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] worker prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var ST = new WeakMap(), BY_ID = new Map();
    var st = function (o) {
        var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var navSnapshot = function () {
        var n = globalThis.navigator || {};
        try {
            return JSON.stringify({ userAgent: n.userAgent, appName: n.appName, appVersion: n.appVersion, platform: n.platform,
                language: n.language, languages: n.languages ? Array.from(n.languages) : undefined, hardwareConcurrency: n.hardwareConcurrency });
        } catch (e) { return '{}'; }
    };
    class Worker extends EventTarget {
        constructor(url, options) {
            if (arguments.length < 1) { throw new TypeError("Failed to construct 'Worker': 1 argument required, but only 0 present."); }
            var o = options === undefined || options === null ? {} : options;
            if (typeof o !== 'object' && typeof o !== 'function') {
                throw new TypeError("Failed to construct 'Worker': The provided value is not of type 'WorkerOptions'.");
            }
            var type = o.type === undefined ? 'classic' : String(o.type);
            if (type !== 'classic' && type !== 'module') {
                throw new TypeError("Failed to construct 'Worker': Failed to read the 'type' property from 'WorkerOptions': The provided value '" + type + "' is not a valid enum value of type WorkerType.");
            }
            if (o.credentials !== undefined && ['omit', 'same-origin', 'include'].indexOf(String(o.credentials)) < 0) {
                throw new TypeError("Failed to construct 'Worker': The provided value '" + o.credentials + "' is not a valid enum value of type RequestCredentials.");
            }
            var name = o.name === undefined ? '' : String(o.name);
            super();
            EventTarget.prototype.addEventListener.call(this, 'message', __msg_stash);
            var id = __worker('spawn', String(url), type, name, navSnapshot());
            ST.set(this, { id: id, terminated: false });
            BY_ID.set(id, this);
        }
        postMessage(message, transfer) {
            var s = st(this);
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'postMessage' on 'Worker': 1 argument required, but only 0 present."); }
            var pkt = __msg_pack(message, __msg_xfer(transfer, 'Worker'));
            if (s.terminated) { return; }
            __worker('post', s.id, pkt.data, pkt.ports);
        }
        terminate() {
            var s = st(this);
            if (s.terminated) { return; }
            s.terminated = true;
            BY_ID.delete(s.id);
            __worker('terminate', s.id);
        }
    }
    __net_handlers(Worker.prototype, ['message', 'messageerror', 'error']);
    Object.defineProperty(Worker.prototype, Symbol.toStringTag, { value: 'Worker', configurable: true });
    hide('Worker', Worker);
    __inbox_on('worker-message', function (m) {
        var w = BY_ID.get(m.id);
        if (!w) { return; }
        __msg_dispatch(w, m.data, m.ports);
    });
    __inbox_on('worker-error', function (m) {
        var w = BY_ID.get(m.id);
        if (!w) { return; }
        if (m.simple) {
            console.error(m.message);
            __net_fire(w, 'Event', 'error', undefined);
            return;
        }
        __net_fire(w, 'ErrorEvent', 'error', { message: m.message, filename: m.filename, lineno: m.lineno, colno: m.colno });
        console.error(m.message + ' (worker ' + m.filename + ':' + m.lineno + ')');
    });
})();
"#;
