//! `WebSocket` (WHATWG WebSockets Standard) over gneiss_pal's RFC 6455 client (AETHERNET, LEDGER SR70).
//!
//! The worker half: [`spawn`] connects on its own thread (`gneiss_pal::api::http::ws`: the opening handshake
//! with the page's Origin and the shared jar's cookies, `permessage-deflate` declined), then loops — the
//! page's commands (send text / binary, close) from a channel, incoming messages from the socket — and
//! posts `open`, `text` / `binary`, `sent`, `error` and the final `close(wasClean, code, reason)` into
//! the page's network queue. Pings are answered inside the client; a server's close frame is echoed.
//!
//! The page half is [`PRELUDE`]: the `WebSocket` interface (url, readyState, protocol, extensions,
//! bufferedAmount, binaryType, send, close, the four event handlers) as an `EventTarget` subclass.

use crate::js::net::{Cmd, NetArg, Poster};
use gneiss_pal::api::http::ws::{self, Message, WsError};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// How long the client waits for the server's answer to its close frame (and then for the TCP close)
/// before it drops the connection itself (RFC 6455 §7.1.1).
const CLOSE_WAIT: Duration = Duration::from_secs(3);

/// One read slice: the loop alternates between the page's commands and the socket this often.
const SLICE: Duration = Duration::from_millis(5);

/// Connects to `url` on its own thread and runs the connection until it closes.
pub fn spawn(url: String, protocols: Vec<String>, origin: String, p: Poster, cmds: Receiver<Cmd>) {
    std::thread::spawn(move || run(url, protocols, origin, p, cmds));
}

fn closed(p: &Poster, clean: bool, code: u16, reason: &str) {
    p.finish("close", vec![NetArg::Bool(clean), NetArg::Num(code as f64), NetArg::Str(reason.to_string())]);
}

/// WebSockets §4 "obtain a WebSocket connection": at most one connection per host and port is in the
/// CONNECTING state; the next one waits until it is established or has failed.
fn connecting() -> &'static (Mutex<HashSet<String>>, Condvar) {
    static C: OnceLock<(Mutex<HashSet<String>>, Condvar)> = OnceLock::new();
    C.get_or_init(|| (Mutex::new(HashSet::new()), Condvar::new()))
}

fn host_key(url: &str) -> String {
    gneiss_pal::api::http::Url::parse(url).map(|u| format!("{}:{}", u.hostname(), u.port_or_default().unwrap_or(0))).unwrap_or_default()
}

fn run(url: String, protocols: Vec<String>, origin: String, p: Poster, cmds: Receiver<Cmd>) {
    let cfg = ws::Config {
        protocols,
        headers: vec![("Origin".into(), origin), ("User-Agent".into(), "UnaOS Aether/0.1.0".into())],
        connect_timeout: Duration::from_secs(15),
        cookies: Some(crate::net::cookie_jar()),
        // Set-Cookie from the handshake is applied on the page's event loop with the `open` task.
        store_cookies: false,
    };
    let key = host_key(&url);
    {
        let (m, cv) = connecting();
        let mut busy = m.lock().unwrap_or_else(|e| e.into_inner());
        while busy.contains(&key) {
            if p.cancelled() {
                return;
            }
            busy = cv.wait_timeout(busy, Duration::from_millis(20)).unwrap_or_else(|e| e.into_inner()).0;
        }
        busy.insert(key.clone());
    }
    let result = ws::connect(&url, &cfg);
    {
        let (m, cv) = connecting();
        m.lock().unwrap_or_else(|e| e.into_inner()).remove(&key);
        cv.notify_all();
    }
    let mut c = match result {
        Ok(c) => c,
        Err(e) => {
            crate::ledger::record_js(&format!("websocket-connect-failed:{}", crate::js::clip(&e.to_string(), 48)));
            p.post("error", vec![NetArg::Str(e.to_string())]);
            closed(&p, false, 1006, "");
            return;
        }
    };
    if !p.post("open", vec![NetArg::Str(c.protocol.clone()), NetArg::Str(c.extensions.clone()), NetArg::List(c.set_cookies.clone())]) {
        let _ = c.close(Some(1001), "");
        c.shutdown();
        return;
    }
    // When this side sent its close frame, and the peer's close (code, reason) once it arrives.
    let mut closing_since: Option<Instant> = None;
    let mut peer_close: Option<(u16, String)> = None;
    loop {
        // The page's commands first (in order), then one read slice.
        loop {
            match cmds.try_recv() {
                Ok(Cmd::WsText(t)) => {
                    let n = t.len();
                    if c.close_sent() {
                        continue;
                    }
                    if c.send_text(&t).is_err() {
                        break;
                    }
                    p.post("sent", vec![NetArg::Num(n as f64)]);
                }
                Ok(Cmd::WsBinary(b)) => {
                    let n = b.len();
                    if c.close_sent() {
                        continue;
                    }
                    if c.send_binary(&b).is_err() {
                        break;
                    }
                    p.post("sent", vec![NetArg::Num(n as f64)]);
                }
                Ok(Cmd::WsClose(code, reason)) => {
                    let _ = c.close(code, &reason);
                    closing_since.get_or_insert_with(Instant::now);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // The page went away (navigation / teardown): going away, 1001.
                    let _ = c.close(Some(1001), "");
                    c.shutdown();
                    return;
                }
            }
        }
        if p.cancelled() {
            let _ = c.close(Some(1001), "");
            c.shutdown();
            return;
        }
        match c.read(SLICE) {
            Ok(Some(Message::Text(t))) => {
                if !p.post("text", vec![NetArg::Str(t)]) {
                    let _ = c.close(Some(1001), "");
                    c.shutdown();
                    return;
                }
            }
            Ok(Some(Message::Binary(b))) => {
                if !p.post("binary", vec![NetArg::Bytes(b)]) {
                    let _ = c.close(Some(1001), "");
                    c.shutdown();
                    return;
                }
            }
            Ok(Some(Message::Ping(_))) | Ok(Some(Message::Pong(_))) => {}
            Ok(Some(Message::Close(pc))) => {
                // The closing handshake is complete once both close frames crossed (the client echoed it
                // in `read` if the server started). Wait briefly for the server to drop TCP first.
                peer_close = Some(pc.unwrap_or((1005, String::new())));
                closing_since.get_or_insert_with(Instant::now);
                let end = Instant::now() + Duration::from_millis(500);
                while Instant::now() < end {
                    match c.read(SLICE) {
                        Ok(_) => {}
                        Err(_) => break,
                    }
                }
                c.shutdown();
                let (code, reason) = peer_close.take().unwrap_or((1005, String::new()));
                closed(&p, true, code, &reason);
                return;
            }
            Ok(None) => {
                if closing_since.is_some_and(|t| t.elapsed() > CLOSE_WAIT) {
                    c.shutdown();
                    p.post("error", vec![NetArg::Str("closing handshake timed out".into())]);
                    closed(&p, false, 1006, "");
                    return;
                }
            }
            Err(WsError::Protocol { code, msg }) => {
                crate::ledger::record_js(&format!("websocket-failed:{code}:{}", crate::js::clip(&msg, 40)));
                p.post("error", vec![NetArg::Str(msg)]);
                closed(&p, false, 1006, "");
                return;
            }
            Err(WsError::Io(m)) => {
                c.shutdown();
                match peer_close.take() {
                    Some((code, reason)) => closed(&p, true, code, &reason),
                    None => {
                        p.post("error", vec![NetArg::Str(m)]);
                        closed(&p, false, 1006, "");
                    }
                }
                return;
            }
        }
    }
}

/// The `WebSocket` interface over `__net_ws_open` / `__net_ws_send` / `__net_ws_close`.
pub const PRELUDE: &str = r#"
(function () {
    var ST = new WeakMap();
    var st = function (o) {
        var s = (o !== null && typeof o === 'object') ? ST.get(o) : undefined;
        if (s === undefined) { throw new TypeError('Illegal invocation'); }
        return s;
    };
    var TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
    var baseURL = function () { try { return document.baseURI; } catch (e) { return location.href; } };
    var utf8len = function (s) { return new TextEncoder().encode(s).length; };
    var syntax = function (m) { return new DOMException("Failed to construct 'WebSocket': " + m, 'SyntaxError'); };
    class WebSocket extends EventTarget {
        constructor(url, protocols) {
            if (arguments.length < 1) {
                throw new TypeError("Failed to construct 'WebSocket': 1 argument required, but only 0 present.");
            }
            var u;
            try { u = new URL(String(url), baseURL()); }
            catch (e) { throw syntax("The URL '" + String(url) + "' is invalid."); }
            if (u.protocol === 'http:') { u.protocol = 'ws:'; }
            else if (u.protocol === 'https:') { u.protocol = 'wss:'; }
            if (u.protocol !== 'ws:' && u.protocol !== 'wss:') {
                throw syntax("The URL's scheme must be either 'http', 'https', 'ws', or 'wss'. '" + u.protocol.slice(0, -1) + "' is not allowed.");
            }
            var href = u.href;
            if (href.indexOf('#') >= 0) {
                throw syntax("The URL contains a fragment identifier ('" + href.slice(href.indexOf('#')) + "'). Fragment identifiers are not allowed in WebSocket URLs.");
            }
            // (DOMString or sequence<DOMString> or WebSocketInit): a non-iterable object is the option bag.
            var list, space = null;
            if (protocols !== null && typeof protocols === 'object' && typeof protocols[Symbol.iterator] !== 'function') {
                var bag = protocols;
                protocols = bag.protocols;
                if (bag.targetAddressSpace !== undefined) {
                    space = String(bag.targetAddressSpace);
                    if (['loopback', 'local', 'public'].indexOf(space) < 0) {
                        throw new TypeError("Failed to construct 'WebSocket': The provided value '" + space + "' is not a valid enum value of type IPAddressSpace.");
                    }
                }
            }
            if (protocols === undefined) { list = []; }
            else if (typeof protocols === 'string' || protocols === null || typeof protocols !== 'object' || typeof protocols[Symbol.iterator] !== 'function') {
                list = [String(protocols)];
            } else {
                list = Array.from(protocols, function (p) { return String(p); });
            }
            var seen = [];
            list.forEach(function (p) {
                if (!TOKEN.test(p)) { throw syntax("The subprotocol '" + p + "' is invalid."); }
                var l = p.toLowerCase();
                if (seen.indexOf(l) >= 0) { throw syntax("The subprotocol '" + p + "' is duplicated."); }
                seen.push(l);
            });
            super();
            var self = this;
            var s = { url: href, origin: u.origin, readyState: 0, protocol: '', extensions: '', buffered: 0, binaryType: 'blob', id: 0, errored: false, closed: false };
            ST.set(this, s);
            if (space !== null && space !== addressSpace(u.hostname)) {
                // Local Network Access: the target is not in the address space the page declared.
                setTimeout(function () { fail(self); }, 0);
                return;
            }
            s.id = __net_ws_open(href, list);
            if (!s.id) {
                setTimeout(function () { fail(self); }, 0);
                return;
            }
            __net_register(s.id, { __net: function (kind, a) { onNet(self, kind, a); } });
        }
        get url() { return st(this).url; }
        get readyState() { return st(this).readyState; }
        get bufferedAmount() { return st(this).buffered; }
        get extensions() { return st(this).extensions; }
        get protocol() { return st(this).protocol; }
        get binaryType() { return st(this).binaryType; }
        set binaryType(v) {
            v = String(v);
            if (v === 'blob' || v === 'arraybuffer') { st(this).binaryType = v; }
        }
        send(data) {
            var s = st(this);
            if (arguments.length < 1) { throw new TypeError("Failed to execute 'send' on 'WebSocket': 1 argument required, but only 0 present."); }
            if (s.readyState === 0) { throw new DOMException("Failed to execute 'send' on 'WebSocket': Still in CONNECTING state.", 'InvalidStateError'); }
            var payload;
            if (data instanceof ArrayBuffer || ArrayBuffer.isView(data)) { payload = data; }
            else if (typeof Blob !== 'undefined' && data instanceof Blob) { payload = typeof __blob_bytes === 'function' ? __blob_bytes(data) : new Uint8Array(0); }
            else { payload = String(data); }
            if (s.readyState !== 1) {
                s.buffered += typeof payload === 'string' ? utf8len(payload) : payload.byteLength;
                return;
            }
            s.buffered += __net_ws_send(s.id, payload);
        }
        close(code, reason) {
            var s = st(this);
            var c;
            if (code !== undefined) {
                var n = Number(code);
                n = isNaN(n) ? 0 : Math.min(Math.max(n, 0), 65535);
                var r = Math.round(n);
                if (Math.abs(n - Math.trunc(n)) === 0.5) { r = Math.trunc(n) % 2 === 0 ? Math.trunc(n) : Math.trunc(n) + 1; }
                c = r;
                if (!(c === 1000 || (c >= 3000 && c <= 4999))) {
                    throw new DOMException("Failed to execute 'close' on 'WebSocket': The close code must be either 1000, or between 3000 and 4999. " + c + ' is neither.', 'InvalidAccessError');
                }
            }
            var why = reason === undefined ? '' : String(reason);
            if (utf8len(why) > 123) {
                throw new DOMException("Failed to execute 'close' on 'WebSocket': The message must not be greater than 123 bytes.", 'SyntaxError');
            }
            if (s.readyState === 2 || s.readyState === 3) { return; }
            if (s.readyState === 0) {
                // "Fail the WebSocket connection" (no handshake has completed).
                s.readyState = 2;
                if (s.id) { __net_cancel(s.id); __net_forget(s.id); }
                var self = this;
                setTimeout(function () { fail(self); }, 0);
                return;
            }
            s.readyState = 2;
            __net_ws_close(s.id, c === undefined && why !== '' ? 1000 : c, why);
        }
    }
    var addressSpace = function (host) {
        host = host.replace(/^\[|\]$/g, '').toLowerCase();
        if (host === 'localhost' || /\.localhost$/.test(host) || /^127\./.test(host) || host === '::1') { return 'loopback'; }
        if (/^10\./.test(host) || /^192\.168\./.test(host) || /^172\.(1[6-9]|2\d|3[01])\./.test(host) || /^169\.254\./.test(host) || /^f[cd]/.test(host) || /^fe80:/.test(host)) { return 'local'; }
        return 'public';
    };
    // "The WebSocket connection is closed" after a failure: readyState CLOSED, then error, then close.
    var fail = function (ws) {
        var s = st(ws);
        if (s.closed) { return; }
        s.readyState = 3;
        if (!s.errored) { s.errored = true; __net_fire(ws, 'Event', 'error', undefined); }
        s.closed = true;
        __net_fire(ws, 'CloseEvent', 'close', { wasClean: false, code: 1006, reason: '' });
    };
    var onNet = function (ws, kind, a) {
        var s = st(ws);
        if (kind === 'open') {
            if (s.readyState !== 0) { return; }
            s.readyState = 1;
            s.protocol = a[0];
            s.extensions = a[1];
            // Cookies the handshake response set land now, on the event loop (not while a script runs).
            if (a[2] && a[2].length) { __net_ws_cookies(s.url, a[2]); }
            __net_fire(ws, 'Event', 'open', undefined);
        } else if (kind === 'text' || kind === 'binary') {
            if (s.readyState !== 1) { return; }
            var data = a[0];
            if (kind === 'binary' && s.binaryType === 'blob') { data = new Blob([data]); }
            __net_fire(ws, 'MessageEvent', 'message', { data: data, origin: s.origin, lastEventId: '' });
        } else if (kind === 'sent') {
            s.buffered = Math.max(0, s.buffered - a[0]);
        } else if (kind === 'error') {
            // A failed connection: readyState is CLOSED when `error` fires (the close event follows).
            if (s.closed || s.errored) { return; }
            s.errored = true;
            s.readyState = 3;
            __net_fire(ws, 'Event', 'error', undefined);
        } else if (kind === 'close') {
            if (s.closed) { return; }
            s.readyState = 3;
            s.closed = true;
            __net_fire(ws, 'CloseEvent', 'close', { wasClean: a[0], code: a[1], reason: a[2] });
        }
    };
    ['CONNECTING', 'OPEN', 'CLOSING', 'CLOSED'].forEach(function (n, i) {
        Object.defineProperty(WebSocket, n, { value: i, enumerable: true });
        Object.defineProperty(WebSocket.prototype, n, { value: i, enumerable: true });
    });
    ['url', 'readyState', 'bufferedAmount', 'extensions', 'protocol', 'binaryType'].forEach(function (n) {
        var d = Object.getOwnPropertyDescriptor(WebSocket.prototype, n);
        d.enumerable = true;
        Object.defineProperty(WebSocket.prototype, n, d);
    });
    ['send', 'close'].forEach(function (n) {
        var d = Object.getOwnPropertyDescriptor(WebSocket.prototype, n);
        d.enumerable = true;
        Object.defineProperty(WebSocket.prototype, n, d);
    });
    __net_handlers(WebSocket.prototype, ['open', 'error', 'close', 'message']);
    Object.defineProperty(WebSocket.prototype, Symbol.toStringTag, { value: 'WebSocket', configurable: true });
    Object.defineProperty(globalThis, 'WebSocket', { value: WebSocket, writable: true, configurable: true, enumerable: false });
})();
"#;
