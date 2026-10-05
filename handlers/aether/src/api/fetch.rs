use js_core::vm::*;
use crate::js::idl;
use std::cell::Cell;

thread_local! {
    /// Per-page fetch budget; reset when a new JS engine boots.
    static FETCH_COUNT: Cell<u32> = const { Cell::new(0) };
}

const FETCH_CAP: u32 = 20;
const BODY_CAP: usize = 5 * 1024 * 1024;

pub fn reset_budget() {
    FETCH_COUNT.with(|c| c.set(0));
}

/// Synchronous HTTP for the JS `fetch` wrapper. Runs the request on a
/// scoped thread with its own blocking client (the engine thread sits
/// inside a tokio runtime, which forbids blocking directly). Fetch-then-
/// apply semantics: page boot blocks on its own requests, like the rest
/// of the load path.
fn native_fetch(url: &str, method: &str, body: &str) -> Option<(u16, String, String)> {
    let n = FETCH_COUNT.with(|c| {
        let v = c.get();
        c.set(v + 1);
        v
    });
    if n >= FETCH_CAP {
        crate::ledger::record_js("fetch-cap-reached");
        return None;
    }
    let base = crate::images::page_base();
    let abs = crate::images::resolve(&base, url);
    if !(abs.starts_with("http://") || abs.starts_with("https://")) {
        crate::ledger::record_js("fetch-bad-url");
        return None;
    }
    let method = method.to_ascii_uppercase();
    let body = body.to_string();
    let abs_thread = abs.clone();
    let result = std::thread::spawn(move || -> Option<(u16, String)> {
        // Shared jar: JS-initiated requests carry and record the same
        // cookies as the page load that spawned them.
        let client = crate::net::blocking_client_builder()
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .ok()?;
        let req = match method.as_str() {
            "POST" => client.post(&abs_thread).body(body),
            _ => client.get(&abs_thread),
        };
        let resp = req.send().ok()?;
        let status = resp.status().as_u16();
        let text = resp.text().ok()?;
        if text.len() > BODY_CAP {
            return None;
        }
        Some((status, text))
    })
    .join()
    .ok()
    .flatten();
    match result {
        Some((status, text)) => Some((status, abs, text)),
        None => {
            crate::ledger::record_js(&format!("fetch-failed:{}", &abs[..abs.len().min(48)]));
            None
        }
    }
}

fn native_fetch_js(vm: &mut Vm, ctx: &CallCtx) -> JsResult<Value> {
    let a0 = vm.arg(ctx, 0);
    let url = vm.to_string(&a0)?.to_rust();
    let a1 = vm.arg(ctx, 1);
    let method = if a1.is_undefined() { String::new() } else { vm.to_string(&a1)?.to_rust() };
    let a2 = vm.arg(ctx, 2);
    let body = if a2.is_undefined() { String::new() } else { vm.to_string(&a2)?.to_rust() };
    match native_fetch(&url, &method, &body) {
        Some((status, final_url, text)) => {
            let o = vm.new_plain_object();
            vm.create_data_property(o, PropertyKey::from_str("status"), Value::Number(status as f64))?;
            vm.create_data_property(o, PropertyKey::from_str("url"), idl::s(&final_url))?;
            vm.create_data_property(o, PropertyKey::from_str("body"), idl::s(&text))?;
            Ok(Value::Object(o))
        }
        None => Ok(Value::Null),
    }
}

/// Installs `__native_fetch` and the WHATWG-shaped `fetch` / `XMLHttpRequest` wrapper over it.
pub fn init(vm: &mut Vm) {
    let f = vm.make_native("__native_fetch", 3, native_fetch_js, false);
    let g = vm.realm().global;
    vm.heap.get_mut(g).props.insert(PropertyKey::from_str("__native_fetch"), Prop::data(Value::Object(f), WC));
    if let Err(e) = vm.run_script_str(PRELUDE) {
        let m = vm.error_string(&e);
        crate::ledger::record_js(&format!("fetch-prelude-failed:{m}"));
    }
}

/// The whatwg-shaped wrapper: promises, Response.text/json, XHR over the same native layer.
const PRELUDE: &str = r#"
        globalThis.fetch = function (url, opts) {
            var method = (opts && opts.method) ? String(opts.method) : 'GET';
            var reqBody = (opts && opts.body != null) ? String(opts.body) : '';
            var r = __native_fetch(String(url), method, reqBody);
            if (!r) {
                return Promise.reject(new TypeError('fetch failed'));
            }
            var resp = {
                ok: r.status >= 200 && r.status < 300,
                status: r.status,
                url: r.url,
                headers: { get: function () { return null; } },
                text: function () { return Promise.resolve(r.body); },
                json: function () {
                    try { return Promise.resolve(JSON.parse(r.body)); }
                    catch (e) { return Promise.reject(e); }
                },
            };
            return Promise.resolve(resp);
        };

        // XMLHttpRequest over the same native layer (sync completion —
        // handlers fire from send(), matching our fetch-then-apply model).
        globalThis.XMLHttpRequest = function () {
            this.readyState = 0;
            this.status = 0;
            this.responseText = '';
            this.response = '';
            this._listeners = {};
        };
        XMLHttpRequest.prototype.open = function (method, url) {
            this._method = String(method || 'GET');
            this._url = String(url || '');
            this.readyState = 1;
        };
        XMLHttpRequest.prototype.setRequestHeader = function () {};
        XMLHttpRequest.prototype.getResponseHeader = function () { return null; };
        XMLHttpRequest.prototype.addEventListener = function (ev, cb) {
            (this._listeners[ev] = this._listeners[ev] || []).push(cb);
        };
        XMLHttpRequest.prototype.send = function (body) {
            var r = __native_fetch(this._url, this._method, body != null ? String(body) : '');
            if (r) {
                this.status = r.status;
                this.responseText = r.body;
                this.response = r.body;
            }
            this.readyState = 4;
            var self_ = this;
            var fire = function (name) {
                if (typeof self_['on' + name] === 'function') { try { self_['on' + name](); } catch (e) {} }
                var ls = self_._listeners[name] || [];
                for (var i = 0; i < ls.length; i++) { try { ls[i].call(self_); } catch (e) {} }
            };
            fire('readystatechange');
            fire(r ? 'load' : 'error');
            fire('loadend');
        };
        "#;
