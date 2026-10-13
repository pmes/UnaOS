//! AETHERSTORAGE (LEDGER SR94): `navigator.serviceWorker` — ServiceWorkerContainer, ServiceWorkerRegistration,
//! ServiceWorker — over `crate::workers::sw`.
//!
//! CHARTER: Aether — binding.
//!
//! `register()` runs the whole job (fetch, evaluate, install, activate-or-wait) synchronously on the page
//! thread inside the native call and settles its promise from the outcome; the registration's `updatefound`
//! and the worker's `statechange` follow as tasks. `controller` is decided once per document (see
//! `workers::sw`); `ready` resolves with the registration whose scope covers the page once one has an active
//! worker. `controllerchange` (a `clients.claim()` or a `skipWaiting()` takeover) and the worker's
//! `client.postMessage` arrive through the `js::broadcast` inbox. `[SecureContext]`: `navigator.serviceWorker`
//! is `undefined` in an insecure context.

use super::idl::{arg, s};
use js_core::vm::*;
use serde_json::{json, Value as J};

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

/// `__sw(op, json)` → JSON text (`{"ok": …}` or `{"e": name, "m": message}`).
fn sw_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    use crate::workers::sw as w;
    let op = str_arg(vm, ctx, 0)?;
    let a: J = serde_json::from_str(&str_arg(vm, ctx, 1)?).unwrap_or(J::Null);
    if op == "secure" {
        return Ok(Value::Bool(crate::storage::page_is_secure()));
    }
    let page = crate::js::page_url();
    let Some(key) = crate::storage::page_storage_key() else {
        return Ok(s(&json!({ "e": "SecurityError", "m": "Service workers are not available in this context." }).to_string()));
    };
    let doc = super::broadcast::doc();
    let st = |k: &str| a.get(k).and_then(J::as_str).unwrap_or("").to_string();
    let out: Result<J, (&'static str, String)> = match op.as_str() {
        "register" => {
            let scope = a.get("scope").and_then(J::as_str).map(str::to_string);
            let uvc = a.get("updateViaCache").and_then(J::as_str).unwrap_or("imports").to_string();
            w::register(&key, &page, &st("scriptURL"), scope.as_deref(), &uvc)
        }
        "update" => w::update(&key, &page, &st("scope")),
        "unregister" => Ok(json!(w::unregister(&key, &st("scope")))),
        "get" => {
            let u = a.get("url").and_then(J::as_str).map(str::to_string).unwrap_or_else(|| page.clone());
            Ok(w::get_registration(&key, &u))
        }
        "list" => Ok(w::registrations(&key)),
        "controller" => Ok(w::controller_json(doc, &key, &page)),
        "post" => Ok(json!(w::post_message(&key, &st("scope"), &st("data"), doc, &page))),
        _ => Err(("TypeError", format!("__sw: unknown operation {op}"))),
    };
    let j = match out {
        Ok(v) => json!({ "ok": v }),
        Err((name, m)) => json!({ "e": name, "m": m }),
    };
    Ok(s(&j.to_string()))
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__sw", 2, sw_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__sw"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("sw-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] service worker prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var TOKEN = {};
    var call = function (op, args) {
        var r = JSON.parse(__sw(op, JSON.stringify(args || {})));
        if (r && r.e !== undefined) { throw r.e === 'TypeError' ? new TypeError(r.m) : new DOMException(r.m, r.e); }
        return r.ok;
    };
    var promised = function (f) { return new Promise(function (res, rej) { try { res(f()); } catch (e) { rej(e); } }); };
    var baseURL = function () { try { return document.baseURI; } catch (e) { return location.href; } };

    var SW = new WeakMap();
    class ServiceWorker extends EventTarget {
        constructor(tok, scope, url, state) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            SW.set(this, { scope: scope, url: url, state: state });
        }
        get scriptURL() { return SW.get(this).url; }
        get state() { return SW.get(this).state; }
        postMessage(message) {
            var s = SW.get(this);
            if (s.state === 'redundant') { throw new DOMException("Failed to execute 'postMessage' on 'ServiceWorker': The worker is redundant.", 'InvalidStateError'); }
            call('post', { scope: s.scope, data: __sc_ser(message) });
        }
    }
    __net_handlers(ServiceWorker.prototype, ['statechange', 'error']);

    var REGS = new Map(), RS = new WeakMap();
    var worker = function (r, which, url, state) {
        var s = RS.get(r);
        if (!url) { s[which] = null; return null; }
        if (!s[which] || SW.get(s[which]).url !== url) { s[which] = new ServiceWorker(TOKEN, s.scope, url, state); }
        SW.get(s[which]).state = state;
        return s[which];
    };
    var setState = function (w, state) {
        if (!w || SW.get(w).state === state) { return; }
        SW.get(w).state = state;
        __net_fire(w, 'Event', 'statechange', undefined);
    };
    class ServiceWorkerRegistration extends EventTarget {
        constructor(tok, scope) {
            if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); }
            super();
            RS.set(this, { scope: scope, active: null, waiting: null, installing: null, uvc: 'imports' });
        }
        get scope() { return RS.get(this).scope; }
        get installing() { return RS.get(this).installing; }
        get waiting() { return RS.get(this).waiting; }
        get active() { return RS.get(this).active; }
        get updateViaCache() { return RS.get(this).uvc; }
        get navigationPreload() {
            return { enable: function () { return Promise.resolve(); }, disable: function () { return Promise.resolve(); },
                setHeaderValue: function () { return Promise.resolve(); }, getState: function () { return Promise.resolve({ enabled: false, headerValue: 'true' }); } };
        }
        update() {
            var self = this;
            return promised(function () { apply(call('update', { scope: RS.get(self).scope })); return self; });
        }
        unregister() {
            var s = RS.get(this);
            return promised(function () {
                var ok = call('unregister', { scope: s.scope });
                if (ok) { setState(s.active, 'redundant'); setState(s.waiting, 'redundant'); REGS.delete(s.scope); }
                return ok;
            });
        }
    }
    __net_handlers(ServiceWorkerRegistration.prototype, ['updatefound']);
    // A registration record from the engine → the (one per scope) registration object, brought up to date.
    var regOf = function (j) {
        if (j === null || j === undefined) { return undefined; }
        var r = REGS.get(j.scope);
        if (!r) { r = new ServiceWorkerRegistration(TOKEN, j.scope); REGS.set(j.scope, r); }
        var s = RS.get(r);
        s.uvc = j.updateViaCache || 'imports';
        worker(r, 'active', j.active ? j.scriptURL : null, 'activated');
        worker(r, 'waiting', j.waiting || null, 'installed');
        return r;
    };
    // A finished register/update job → the registration, with its events as tasks.
    var apply = function (out) {
        var j = out.registration;
        var r = regOf(j);
        if (out.installed) {
            var s = RS.get(r);
            setTimeout(function () {
                __net_fire(r, 'Event', 'updatefound', undefined);
                if (out.activated) {
                    var a = s.active;
                    if (a) { SW.get(a).state = 'installed'; setState(a, 'activating'); setState(a, 'activated'); }
                } else if (s.waiting) {
                    setState(s.waiting, 'installed');
                }
                flushReady();
            }, 0);
        }
        return r;
    };

    var controller = null, controllerScope = null, readyWaiters = [];
    var refreshController = function () {
        var j = call('controller', {});
        if (j === null) { controller = null; controllerScope = null; return null; }
        var r = regOf(j);
        controllerScope = j.scope;
        controller = RS.get(r).active;
        return controller;
    };
    var flushReady = function () {
        if (!readyWaiters.length) { return; }
        var r = regOf(call('get', { url: location.href }));
        if (r && RS.get(r).active) {
            var w = readyWaiters; readyWaiters = [];
            w.forEach(function (f) { f(r); });
        }
    };
    var decided = false;
    class ServiceWorkerContainer extends EventTarget {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } super(); }
        get controller() {
            if (!decided) { decided = true; try { refreshController(); } catch (e) {} }
            return controller;
        }
        get ready() {
            return new Promise(function (res) { readyWaiters.push(res); setTimeout(flushReady, 0); });
        }
        register(scriptURL, options) {
            var o = options || {};
            return promised(function () {
                var url = String(new URL(String(scriptURL), baseURL()));
                var scope = o.scope === undefined ? undefined : String(new URL(String(o.scope), baseURL()));
                if (o.type !== undefined && String(o.type) === 'module') { __ledger('serviceWorker.register-module-as-classic'); }
                var uvc = o.updateViaCache === undefined ? 'imports' : String(o.updateViaCache);
                if (['imports', 'all', 'none'].indexOf(uvc) < 0) { throw new TypeError("The provided value '" + uvc + "' is not a valid enum value of type ServiceWorkerUpdateViaCache."); }
                return apply(call('register', { scriptURL: url, scope: scope, updateViaCache: uvc }));
            });
        }
        getRegistration(clientURL) {
            return promised(function () {
                var u = clientURL === undefined ? location.href : String(new URL(String(clientURL), baseURL()));
                return regOf(call('get', { url: u }));
            });
        }
        getRegistrations() { return promised(function () { return call('list', {}).map(regOf); }); }
        startMessages() {}
    }
    __net_handlers(ServiceWorkerContainer.prototype, ['controllerchange', 'message', 'messageerror']);
    var container = new ServiceWorkerContainer(TOKEN);
    __inbox_on('sw-controllerchange', function (m) {
        decided = true;
        refreshController();
        __net_fire(container, 'Event', 'controllerchange', undefined);
    });
    __inbox_on('sw-message', function (m) {
        var data;
        try { data = __sc_de(m.data); }
        catch (e) { __net_fire(container, 'MessageEvent', 'messageerror', { data: null, origin: m.origin }); return; }
        var src = REGS.get(m.scope);
        __net_fire(container, 'MessageEvent', 'message', { data: data, origin: m.origin, source: src ? RS.get(src).active : null });
    });
    [ServiceWorker, ServiceWorkerRegistration, ServiceWorkerContainer].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(C.name, C);
    });
    var NP = (typeof Navigator === 'function' && Navigator.prototype) || (typeof navigator === 'object' && Object.getPrototypeOf(navigator));
    if (NP) {
        Object.defineProperty(NP, 'serviceWorker', {
            get: function () { return __sw('secure', '{}') === true ? container : undefined; },
            configurable: true, enumerable: true,
        });
    }
})();
"#;
