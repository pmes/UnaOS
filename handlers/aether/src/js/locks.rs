//! AETHERSTORAGE (LEDGER SR94): `navigator.locks` — the Web Locks API's page-facing half (LockManager, Lock)
//! over `crate::storage::locks`.
//!
//! CHARTER: Aether — binding.
//!
//! `request(name, [options,] callback)` asks the manager at once: `granted` runs the callback in a task with a
//! `Lock`, `unavailable` (`ifAvailable`) runs it with `null`, `queued` waits for the inbox's `lock-granted`.
//! The lock is held until the promise the callback returned settles; the returned promise settles the same
//! way. An `AbortSignal` that fires while the request waits takes it out of the queue and rejects with the
//! signal's reason; a `steal` elsewhere rejects the holder's promise with "AbortError" (`lock-stolen`).
//! `[SecureContext]`: `navigator.locks` is `undefined` in an insecure context.

use super::idl::{self, arg, s};
use js_core::vm::*;
use serde_json::{json, Value as J};

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

/// `__locks(op, json)` → JSON text: `request`, `release`, `abort`, `query`, `secure`.
fn locks_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    use crate::storage::locks as l;
    let op = str_arg(vm, ctx, 0)?;
    let a: J = serde_json::from_str(&str_arg(vm, ctx, 1)?).unwrap_or(J::Null);
    if op == "secure" {
        return Ok(Value::Bool(crate::storage::page_is_secure()));
    }
    let Some(key) = crate::storage::page_storage_key() else {
        return idl::throw_dom(vm, "SecurityError", "Access to the Locks API is denied in this context.");
    };
    let id = a.get("id").and_then(J::as_u64).unwrap_or(0);
    let out = match op.as_str() {
        "request" => l::request(
            &key,
            a.get("name").and_then(J::as_str).unwrap_or(""),
            a.get("mode").and_then(J::as_str) != Some("shared"),
            a.get("ifAvailable").and_then(J::as_bool).unwrap_or(false),
            a.get("steal").and_then(J::as_bool).unwrap_or(false),
            super::broadcast::doc(),
        ),
        "release" => {
            l::release(id);
            J::Null
        }
        "abort" => json!(l::abort(id)),
        "query" => l::query(&key),
        _ => return vm.throw_type("__locks: unknown operation"),
    };
    Ok(s(&out.to_string()))
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__locks", 2, locks_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__locks"), Prop::data(Value::Object(fo), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("locks-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] locks prelude failed: {m}");
    }
}

const PRELUDE: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    var TOKEN = {};
    var call = function (op, args) { return JSON.parse(__locks(op, JSON.stringify(args || {}))); };
    var WAIT = new Map(), HELD = new Map();
    var LK = new WeakMap();
    class Lock {
        constructor(tok, name, mode) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } LK.set(this, { name: name, mode: mode }); }
        get name() { return LK.get(this).name; }
        get mode() { return LK.get(this).mode; }
    }
    var notSupported = function (m) { return new DOMException("Failed to execute 'request' on 'LockManager': " + m, 'NotSupportedError'); };
    class LockManager {
        constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
        request(name, options, callback) {
            if (callback === undefined) { callback = options; options = {}; }
            var o = options || {};
            if (typeof callback !== 'function') {
                return Promise.reject(new TypeError("Failed to execute 'request' on 'LockManager': The callback provided as parameter 2 is not a function."));
            }
            name = String(name);
            var mode = o.mode === undefined ? 'exclusive' : String(o.mode);
            if (mode !== 'exclusive' && mode !== 'shared') {
                return Promise.reject(new TypeError("The provided value '" + mode + "' is not a valid enum value of type LockMode."));
            }
            var steal = !!o.steal, ifAvailable = !!o.ifAvailable, signal = o.signal;
            if (name.charAt(0) === '-') { return Promise.reject(notSupported("Names starting with '-' are reserved.")); }
            if (steal && ifAvailable) { return Promise.reject(notSupported("The 'steal' and 'ifAvailable' options cannot be used together.")); }
            if (steal && mode !== 'exclusive') { return Promise.reject(notSupported("The 'steal' option may only be used with 'exclusive' locks.")); }
            if (signal && (steal || ifAvailable)) { return Promise.reject(notSupported("The 'signal' option cannot be used with other options.")); }
            if (signal && signal.aborted) { return Promise.reject(signal.reason); }
            var r;
            try { r = call('request', { name: name, mode: mode, ifAvailable: ifAvailable, steal: steal }); }
            catch (e) { return Promise.reject(e); }
            return new Promise(function (resolve, reject) {
                var run = function (lock) {
                    var settle = function (ok, v) {
                        if (lock && HELD.has(r.id)) { HELD.delete(r.id); call('release', { id: r.id }); }
                        if (ok) { resolve(v); } else { reject(v); }
                    };
                    if (lock) { HELD.set(r.id, { reject: reject }); }
                    var out;
                    try { out = callback(lock); } catch (e) { settle(false, e); return; }
                    Promise.resolve(out).then(function (v) { settle(true, v); }, function (e) { settle(false, e); });
                };
                if (r.state === 'unavailable') { setTimeout(function () { run(null); }, 0); return; }
                var grant = function () { run(new Lock(TOKEN, name, mode)); };
                if (r.state === 'granted') { setTimeout(grant, 0); return; }
                WAIT.set(r.id, grant);
                if (signal) {
                    signal.addEventListener('abort', function () {
                        if (WAIT.has(r.id) && call('abort', { id: r.id }) === true) {
                            WAIT.delete(r.id);
                            reject(signal.reason);
                        }
                    });
                }
            });
        }
        query() {
            return new Promise(function (res, rej) { try { res(call('query', {})); } catch (e) { rej(e); } });
        }
    }
    __inbox_on('lock-granted', function (m) {
        var g = WAIT.get(m.id);
        if (g) { WAIT.delete(m.id); g(); }
    });
    __inbox_on('lock-stolen', function (m) {
        var h = HELD.get(m.id);
        if (h) { HELD.delete(m.id); h.reject(new DOMException('The lock was stolen by another request.', 'AbortError')); }
    });
    [Lock, LockManager].forEach(function (C) {
        Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
        hide(C.name, C);
    });
    var manager = new LockManager(TOKEN);
    var NP = (typeof Navigator === 'function' && Navigator.prototype) || (typeof navigator === 'object' && Object.getPrototypeOf(navigator));
    if (NP) {
        Object.defineProperty(NP, 'locks', { get: function () { return __locks('secure', '{}') === true ? manager : undefined; }, configurable: true, enumerable: true });
    }
})();
"#;
