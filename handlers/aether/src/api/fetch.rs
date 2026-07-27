use boa_engine::{Context, native_function::NativeFunction};
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
        let client = reqwest::blocking::Client::builder()
            .user_agent("UnaOS Aether/0.1.0")
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

pub fn init(context: &mut Context) {
    let f = NativeFunction::from_fn_ptr(|_this, args, ctx| {
        let url = args
            .first()
            .cloned()
            .unwrap_or_default()
            .to_string(ctx)
            .unwrap_or_default()
            .to_std_string_escaped();
        let method = args
            .get(1)
            .cloned()
            .unwrap_or_default()
            .to_string(ctx)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        let body = args
            .get(2)
            .cloned()
            .unwrap_or_default()
            .to_string(ctx)
            .map(|s| s.to_std_string_escaped())
            .unwrap_or_default();
        match native_fetch(&url, &method, &body) {
            Some((status, final_url, text)) => {
                let o = boa_engine::object::ObjectInitializer::new(ctx)
                    .property(boa_engine::string::JsString::from("status"), status as i32, boa_engine::property::Attribute::all())
                    .property(
                        boa_engine::string::JsString::from("url"),
                        boa_engine::string::JsString::from(final_url),
                        boa_engine::property::Attribute::all(),
                    )
                    .property(
                        boa_engine::string::JsString::from("body"),
                        boa_engine::string::JsString::from(text),
                        boa_engine::property::Attribute::all(),
                    )
                    .build();
                Ok(o.into())
            }
            None => Ok(boa_engine::JsValue::null()),
        }
    });
    context.register_global_callable("__native_fetch".into(), 3, f).unwrap();

    // The whatwg-shaped wrapper lives in JS: promises, Response.text/json.
    let _ = context.eval(boa_engine::Source::from_bytes(
        r#"
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
        "#,
    ));
}
