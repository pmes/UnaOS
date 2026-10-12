//! `EventSource` (HTML §9.2 server-sent events) — AETHERNET, LEDGER SR70.
//!
//! The worker half: [`spawn`] runs the connection on its own thread through gneiss_pal::api::http's
//! blocking client (`Accept: text/event-stream`, `Cache-Control: no-cache`, `Last-Event-ID` on a
//! reconnect), parses the body with gneiss_pal's HTML event-stream interpreter
//! (`api::sse::EventStreamDecoder` — the same module whose `SseDecoder` feeds Vein's providers) as each
//! chunk arrives, and posts `open(origin)`, `message(type, data, lastEventId)` and `error(reconnecting)`.
//! A dropped connection is re-established after the reconnection time (`retry:` sets it; 3 s until
//! then); a response that is not `200 text/event-stream` fails the connection for good (`error(false)`
//! is the last message).
//!
//! The page half is [`PRELUDE`]: the `EventSource` interface as an `EventTarget` subclass.

use crate::js::net::{NetArg, Poster};
use gneiss_pal::api::sse::EventStreamDecoder;
use std::time::Duration;

/// The reconnection time before a stream sets one (HTML leaves it to the user agent; engines use 3 s).
pub const DEFAULT_RETRY_MS: u64 = 3_000;

/// Opens `url` on its own thread and keeps it open (reconnecting) until the page closes it.
pub fn spawn(url: String, p: Poster) {
    std::thread::spawn(move || run(url, p));
}

fn is_event_stream(ct: Option<&str>) -> bool {
    ct.map(|v| v.split(';').next().unwrap_or("").trim().eq_ignore_ascii_case("text/event-stream")).unwrap_or(false)
}

fn run(url: String, p: Poster) {
    let mut dec = EventStreamDecoder::new();
    let mut retry = DEFAULT_RETRY_MS;
    loop {
        if p.cancelled() {
            return;
        }
        let client = match crate::net::blocking_client_builder().timeout(Duration::from_secs(300)).build() {
            Ok(c) => c,
            Err(e) => {
                p.finish("error", vec![NetArg::Bool(false), NetArg::Str(e.to_string())]);
                return;
            }
        };
        let mut rb = client.get(&url).header("Accept", "text/event-stream").header("Cache-Control", "no-cache");
        if !dec.last_event_id().is_empty() {
            rb = rb.header("Last-Event-ID", dec.last_event_id());
        }
        let mut resp = match rb.send() {
            Ok(r) => r,
            Err(e) => {
                // A network error: "reestablish the connection".
                if !p.post("error", vec![NetArg::Bool(true), NetArg::Str(e.to_string())]) || !p.sleep(retry) {
                    return;
                }
                continue;
            }
        };
        let status = resp.status().as_u16();
        if status != 200 || !is_event_stream(resp.headers().get("content-type")) {
            // "Fail the connection": no reconnection.
            crate::ledger::record_js(&format!("eventsource-failed:{status}"));
            p.finish("error", vec![NetArg::Bool(false), NetArg::Str(format!("status {status}"))]);
            return;
        }
        let origin = resp.url().origin();
        if !p.post("open", vec![NetArg::Str(origin)]) {
            return;
        }
        dec.reset_stream();
        loop {
            if p.cancelled() {
                return;
            }
            match resp.chunk() {
                Ok(Some(c)) => {
                    for ev in dec.push(&c) {
                        if !p.post("message", vec![NetArg::Str(ev.ty), NetArg::Str(ev.data), NetArg::Str(ev.last_event_id)]) {
                            return;
                        }
                    }
                    if let Some(r) = dec.take_retry() {
                        retry = r;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        }
        if let Some(r) = dec.take_retry() {
            retry = r;
        }
        if !p.post("error", vec![NetArg::Bool(true), NetArg::Str("connection closed".into())]) || !p.sleep(retry.min(24 * 3600 * 1000)) {
            return;
        }
    }
}

/// The `EventSource` interface over `__net_es_open` / `__net_cancel`.
pub const PRELUDE: &str = r#"
(function () {
    var ST = new WeakMap();
    var st = function (o) {
        var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var baseURL = function () { try { return document.baseURI; } catch (e) { return location.href; } };
    class EventSource extends EventTarget {
        constructor(url, init) {
            if (arguments.length < 1) {
                throw new TypeError("Failed to construct 'EventSource': 1 argument required, but only 0 present.");
            }
            var u;
            try { u = new URL(String(url), baseURL()); }
            catch (e) { throw new DOMException("Failed to construct 'EventSource': Cannot open an EventSource to '" + String(url) + "'. The URL is invalid.", 'SyntaxError'); }
            var wc = !!(init && init.withCredentials);
            super();
            var self = this;
            var s = { url: u.href, withCredentials: wc, readyState: 0, id: 0, origin: u.origin };
            ST.set(this, s);
            if (!/^https?:$/.test(u.protocol)) {
                setTimeout(function () { failed(self); }, 0);
                return;
            }
            s.id = __net_es_open(u.href.replace(/#.*$/, ''), wc);
            if (!s.id) {
                setTimeout(function () { failed(self); }, 0);
                return;
            }
            __net_register(s.id, { __net: function (kind, a) { onNet(self, kind, a); } });
        }
        get url() { return st(this).url; }
        get withCredentials() { return st(this).withCredentials; }
        get readyState() { return st(this).readyState; }
        close() {
            var s = st(this);
            if (s.id) { __net_cancel(s.id); __net_forget(s.id); }
            s.readyState = 2;
        }
    }
    var failed = function (es) {
        var s = st(es);
        if (s.readyState === 2) { return; }
        s.readyState = 2;
        __net_fire(es, 'Event', 'error', undefined);
    };
    var onNet = function (es, kind, a) {
        var s = st(es);
        if (s.readyState === 2) { return; }
        if (kind === 'open') {
            s.readyState = 1;
            s.origin = a[0];
            __net_fire(es, 'Event', 'open', undefined);
        } else if (kind === 'message') {
            __net_fire(es, 'MessageEvent', a[0], { data: a[1], origin: s.origin, lastEventId: a[2] });
        } else if (kind === 'error') {
            s.readyState = a[0] ? 0 : 2;
            __net_fire(es, 'Event', 'error', undefined);
        }
    };
    ['CONNECTING', 'OPEN', 'CLOSED'].forEach(function (n, i) {
        Object.defineProperty(EventSource, n, { value: i, enumerable: true });
        Object.defineProperty(EventSource.prototype, n, { value: i, enumerable: true });
    });
    ['url', 'withCredentials', 'readyState', 'close'].forEach(function (n) {
        var d = Object.getOwnPropertyDescriptor(EventSource.prototype, n);
        d.enumerable = true;
        Object.defineProperty(EventSource.prototype, n, d);
    });
    __net_handlers(EventSource.prototype, ['open', 'message', 'error']);
    Object.defineProperty(EventSource.prototype, Symbol.toStringTag, { value: 'EventSource', configurable: true });
    Object.defineProperty(globalThis, 'EventSource', { value: EventSource, writable: true, configurable: true, enumerable: false });
})();
"#;
