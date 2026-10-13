//! AETHERSTORAGE (LEDGER SR94): the Cache API's page-facing half — `caches` (CacheStorage) and `Cache`
//! (Service Workers §5.4–§5.5) over `crate::storage::cache`.
//!
//! CHARTER: Aether — binding.
//!
//! A request is reduced to `{url, method, headers}` (the URL with its fragment removed), a response to
//! `{status, statusText, headers, type, url}` plus its body bytes, read whole (`put` consumes the response,
//! §5.4.3). `match` answers a NEW `Response` built from the stored entry (its `url` and `type` carried as own
//! properties: the Response constructor cannot set them). `add/addAll` fetch with the page's `fetch` and
//! refuse a response that is not ok (TypeError) before storing any of them, so an `addAll` is all or nothing.
//! `[SecureContext]`: in an insecure context `caches` is `undefined`.

use super::idl::{self, arg, s};
use js_core::vm::*;
use serde_json::{json, Value as J};

fn str_arg(vm: &mut Vm, ctx: &CallCtx, i: usize) -> JsResult<String> {
    let v = arg(vm, ctx, i);
    Ok(vm.to_string(&v)?.to_rust())
}

/// `__cache(op, json, body?)` in the page realm.
fn cache_native(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    run(vm, ctx, crate::storage::page_storage_key(), crate::storage::page_is_secure())
}

/// The `__cache` operations for a realm whose storage key is `key` (`None`: an opaque origin, "SecurityError")
/// — the page's (above) and a service worker's (`workers::realm`). Answers JSON text, or for `match` an array
/// of `[entry JSON, ArrayBuffer]`.
pub fn run(vm: &mut Vm, ctx: &CallCtx, key: Option<String>, secure: bool) -> JsResult<Value> {
    use crate::storage::cache as c;
    let op = str_arg(vm, ctx, 0)?;
    let a: J = serde_json::from_str(&str_arg(vm, ctx, 1)?).unwrap_or(J::Null);
    if op == "secure" {
        return Ok(Value::Bool(secure));
    }
    let Some(key) = key else {
        return idl::throw_dom(vm, "SecurityError", "Access to the Cache API is denied in this context.");
    };
    let name = a.get("name").and_then(J::as_str).map(str::to_string);
    let opts = c::QueryOptions::from_json(a.get("options").unwrap_or(&J::Null));
    let fail = |vm: &mut Vm, e: String| -> JsResult<Value> {
        if let Some(m) = e.strip_prefix("NotFoundError: ") {
            return idl::throw_dom(vm, "NotFoundError", m);
        }
        idl::throw_dom(vm, "UnknownError", &e)
    };
    let out = match op.as_str() {
        "open" => match c::open(&key, name.as_deref().unwrap_or("")) {
            Ok(()) => json!(true),
            Err(e) => return fail(vm, e),
        },
        "has" => json!(c::has(&key, name.as_deref().unwrap_or(""))),
        "delete" => match c::delete_cache(&key, name.as_deref().unwrap_or("")) {
            Ok(b) => json!(b),
            Err(e) => return fail(vm, e),
        },
        "keys" => json!(c::cache_names(&key)),
        "put" => {
            let body = crate::js::net::buffer_bytes(vm, &arg(vm, ctx, 2)).unwrap_or_default();
            match c::put(&key, name.as_deref().unwrap_or(""), a.get("req").unwrap_or(&J::Null), a.get("resp").unwrap_or(&J::Null), &body) {
                Ok(()) => json!(true),
                Err(e) => return fail(vm, e),
            }
        }
        "match" => {
            let req = a.get("req").filter(|r| !r.is_null());
            let first = a.get("first").and_then(J::as_bool).unwrap_or(false);
            let found = c::match_all(&key, name.as_deref(), req, opts, first);
            let mut rows = Vec::with_capacity(found.len());
            for (entry, body) in found {
                let buf = crate::js::net::array_buffer(vm, &body)?;
                rows.push(Value::Object(vm.new_array(vec![s(&entry.to_string()), Value::Object(buf)])));
            }
            return Ok(Value::Object(vm.new_array(rows)));
        }
        "deleteEntry" => match c::delete_entries(&key, name.as_deref().unwrap_or(""), a.get("req").unwrap_or(&J::Null), opts) {
            Ok(b) => json!(b),
            Err(e) => return fail(vm, e),
        },
        "requests" => json!(c::request_keys(&key, name.as_deref().unwrap_or(""), a.get("req").filter(|r| !r.is_null()), opts)),
        _ => return vm.throw_type("__cache: unknown operation"),
    };
    Ok(s(&out.to_string()))
}

pub fn install(vm: &mut Vm) {
    let g = vm.realm().global;
    let fo = vm.make_native("__cache", 3, cache_native as NativeFn, false);
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__cache"), Prop::data(Value::Object(fo), WC));
    install_shapes(vm);
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("cache-api-prelude-failed:{}", super::clip(&m, 64)));
        eprintln!("[aether] cache api prelude failed: {m}");
    }
}

/// The CacheStorage / Cache shapes; the service-worker realm runs [`CACHE_SHAPES`] over its own `__cache`.
const PRELUDE: &str = r#"
(function () {
    var install = __cache_shapes;
    var cs = install();
    Object.defineProperty(globalThis, 'caches', {
        get: function () { return __cache('secure', '{}') === true ? cs : undefined; },
        configurable: true, enumerable: true,
    });
})();
"#;

/// The Cache API classes, as a function both realms call (`__cache_shapes()` → the CacheStorage instance).
pub const CACHE_SHAPES: &str = r#"
(function () {
    var hide = function (name, v) {
        Object.defineProperty(globalThis, name, { value: v, writable: true, configurable: true, enumerable: false });
    };
    hide('__cache_shapes', function () {
        var TOKEN = {};
        var call = function (op, args, body) {
            var r = __cache(op, JSON.stringify(args || {}), body);
            return typeof r === 'string' ? JSON.parse(r) : r;
        };
        var headerList = function (h) {
            var out = [];
            if (h && typeof h.forEach === 'function') { h.forEach(function (v, k) { out.push([k, v]); }); }
            return out;
        };
        var toRequest = function (r) {
            if (typeof Request === 'function' && r instanceof Request) { return r; }
            return new Request(String(r));
        };
        var wire = function (r) {
            var q = toRequest(r);
            return { url: String(q.url).split('#')[0], method: q.method, headers: headerList(q.headers) };
        };
        var opts = function (o) {
            o = o || {};
            return { ignoreSearch: !!o.ignoreSearch, ignoreMethod: !!o.ignoreMethod, ignoreVary: !!o.ignoreVary };
        };
        var respond = function (row) {
            var e = JSON.parse(row[0]);
            var st = e.status >= 200 && e.status <= 599 ? e.status : 200;
            var nullBody = [101, 103, 204, 205, 304].indexOf(st) >= 0;
            var r = new Response(nullBody ? null : row[1], { status: st, statusText: e.statusText || '', headers: e.headers || [] });
            Object.defineProperty(r, 'url', { value: e.respUrl || e.url || '', configurable: true });
            Object.defineProperty(r, 'type', { value: e.type || 'default', configurable: true });
            if (st !== e.status) { Object.defineProperty(r, 'status', { value: e.status, configurable: true }); }
            return r;
        };
        var promised = function (f) { return new Promise(function (res, rej) { try { res(f()); } catch (e) { rej(e); } }); };
        var checkPut = function (q, resp) {
            var u = String(q.url);
            if (!/^https?:/i.test(u)) { throw new TypeError("Failed to execute 'put' on 'Cache': Request scheme '" + u.split(':')[0] + "' is unsupported"); }
            if (q.method !== 'GET') { throw new TypeError("Failed to execute 'put' on 'Cache': Request method '" + q.method + "' is unsupported"); }
            if (resp.status === 206) { throw new TypeError("Failed to execute 'put' on 'Cache': Partial response (status code 206) is unsupported"); }
            var vary = resp.headers && resp.headers.get ? resp.headers.get('vary') : null;
            if (vary && vary.split(',').some(function (f) { return f.trim() === '*'; })) {
                throw new TypeError("Failed to execute 'put' on 'Cache': Vary header contains *");
            }
            if (resp.bodyUsed) { throw new TypeError("Failed to execute 'put' on 'Cache': Response body is already used"); }
        };
        var store = function (name, q, resp) {
            var meta = { status: resp.status, statusText: resp.statusText, headers: headerList(resp.headers), type: resp.type, url: resp.url };
            var reqw = { url: String(q.url).split('#')[0], method: q.method, headers: headerList(q.headers) };
            return resp.arrayBuffer().then(function (buf) { call('put', { name: name, req: reqw, resp: meta }, buf); });
        };
        var CN = new WeakMap();
        class Cache {
            constructor(tok, name) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } CN.set(this, name); }
            match(request, options) {
                var name = CN.get(this);
                return promised(function () {
                    var rows = call('match', { name: name, req: wire(request), options: opts(options), first: true });
                    return rows.length ? respond(rows[0]) : undefined;
                });
            }
            matchAll(request, options) {
                var name = CN.get(this);
                return promised(function () {
                    var rows = call('match', { name: name, req: request === undefined ? null : wire(request), options: opts(options) });
                    return rows.map(respond);
                });
            }
            add(request) { return this.addAll([request]); }
            addAll(requests) {
                var name = CN.get(this);
                var list;
                try { list = Array.from(requests, toRequest); } catch (e) { return Promise.reject(e); }
                for (var i = 0; i < list.length; i++) {
                    if (!/^https?:/i.test(String(list[i].url)) || list[i].method !== 'GET') {
                        return Promise.reject(new TypeError("Failed to execute 'addAll' on 'Cache': Request scheme or method is unsupported"));
                    }
                }
                return Promise.all(list.map(function (q) { return fetch(q); })).then(function (resps) {
                    resps.forEach(function (r, i) {
                        if (!r.ok) { throw new TypeError("Failed to execute 'addAll' on 'Cache': Request failed"); }
                        checkPut(list[i], r);
                    });
                    return Promise.all(resps.map(function (r, i) { return store(name, list[i], r); }));
                }).then(function () { return undefined; });
            }
            put(request, response) {
                var name = CN.get(this);
                try {
                    var q = toRequest(request);
                    checkPut(q, response);
                    return store(name, q, response).then(function () { return undefined; });
                } catch (e) { return Promise.reject(e); }
            }
            delete(request, options) {
                var name = CN.get(this);
                return promised(function () { return call('deleteEntry', { name: name, req: wire(request), options: opts(options) }); });
            }
            keys(request, options) {
                var name = CN.get(this);
                return promised(function () {
                    var rows = call('requests', { name: name, req: request === undefined ? null : wire(request), options: opts(options) });
                    return rows.map(function (r) { return new Request(r.url, { method: r.method || 'GET', headers: r.headers || [] }); });
                });
            }
        }
        class CacheStorage {
            constructor(tok) { if (tok !== TOKEN) { throw new TypeError('Illegal constructor'); } }
            open(name) {
                name = String(name);
                return promised(function () { call('open', { name: name }); return new Cache(TOKEN, name); });
            }
            has(name) { name = String(name); return promised(function () { return call('has', { name: name }); }); }
            delete(name) { name = String(name); return promised(function () { return call('delete', { name: name }); }); }
            keys() { return promised(function () { return call('keys', {}); }); }
            match(request, options) {
                var o = options || {};
                return promised(function () {
                    var args = { name: o.cacheName === undefined ? null : String(o.cacheName), req: wire(request), options: opts(o), first: true };
                    if (args.name === null) { delete args.name; }
                    var rows = call('match', args);
                    return rows.length ? respond(rows[0]) : undefined;
                });
            }
        }
        [Cache, CacheStorage].forEach(function (C) {
            Object.defineProperty(C.prototype, Symbol.toStringTag, { value: C.name, configurable: true });
            hide(C.name, C);
        });
        return new CacheStorage(TOKEN);
    });
})();
"#;

/// Installs the shapes (called by [`install`] before its prelude; the service-worker realm runs the same text).
pub fn install_shapes(vm: &mut Vm) {
    if let Err(e) = vm.run_script_str(CACHE_SHAPES) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("cache-shapes-failed:{}", super::clip(&m, 64)));
    }
}
