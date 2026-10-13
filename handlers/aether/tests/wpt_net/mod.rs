//! AETHERNET (SR70): the local stand-in for wptserve that `wpt_dom.rs` runs the `websockets/`,
//! `eventsource/` and `xhr/` directories against. wptserve itself is not available here, so this serves
//! what those tests need and nothing more:
//!
//! - static files, with wptserve's `.headers` sidecar files and its `sub` pipe (`*.sub.*` files and
//!   `?pipe=sub`: `{{host}}`, `{{ports[..][0]}}`, `{{domains[..]}}`, `{{hosts[..][..]}}`,
//!   `{{location[..]}}`, `{{headers[..]}}`, `{{GET[..]}}`);
//! - the generated wrappers for `X.any.js` (window scope) and `X.window.js` tests (`X.any.html`,
//!   `X.window.html`), with their `// META: script=` and `// META: title=` lines;
//! - the Python handlers the three directories call most, re-implemented from their sources at the pinned
//!   WPT commit (`eventsource/resources/{message,message2,last-event-id,last-event-id2,status-error,
//!   status-reconnect,reconnect-fail}.py`, `xhr/resources/{content,status,delay,trickle,inspect-headers,
//!   headers,redirect,echo-method,requri,corsenabled}.py`, `common/redirect.py`) — any other `.py` answers
//!   500 and the runner skips the tests that reference it, by name;
//! - a WebSocket server on its own port standing in for pywebsocket and `websockets/handlers/*_wsh.py`
//!   (echo, echo_exit, echo_raw, echo_close_data, empty-message, echo-query, echo-query_v13, protocol,
//!   protocol_array, handshake_protocol, handshake_no_protocol, handshake_no_extensions, simple_handshake,
//!   wrong_accept_key, invalid, origin, echo-cookie, set-cookie, set-cookie_http, handshake_sleep_2,
//!   referrer, remote-close, delayed-passive-close).
//!
//! AETHERWPT2 (SR135) adds the handlers `fetch/api/` calls most (`fetch/api/resources/{redirect,inspect-headers,
//! clean-stash,stash-put,stash-take,status,method,echo-content,trickle,infinite-slow-response}.py`) and two of
//! `common/` (`slow.py`, `echo.py`), with wptserve's stash (one process-wide map keyed by key and URL directory,
//! `take` removes, a second `put` of a live key is an error) behind the stash-using ones.
//!
//! AETHERWPT5 (SR144): the stand-in serves `/resources/testdriver-vendor.js` ([`TESTDRIVER_VENDOR_JS`]), the
//! WebDriver commands of `testdriver.js` over the engine's automation queue (`js/testdriver.rs`,
//! `AetherEngine::pump_automation`); a second origin: the page origin is `http://localhost:{{ports[http][0]}}`
//! (wptserve's own localhost configuration, so `get-host-info.sub.js` names `127.0.0.1` as the remote and
//! not-same-site host), `{{domains[www]}}` / `{{domains[www1]}}` are `127.0.0.1`, `{{domains[www2]}}` `127.0.0.2`,
//! the `alt` hosts `127.0.0.3`–`127.0.0.6` (every 127/8 address is the loopback on Linux; a listener is bound per
//! address and per port), `{{ports[http][1]}}` a second port, `{{hosts[][nonexistent]}}` a `.invalid` name; and
//! the handlers of `html/semantics/{scripting-1,embedded-content}`, `xhr`, `resource-timing`, `encoding`, `url`,
//! `fetch/api`, `eventsource`, `websockets`, `FileAPI`, `html/browsers` and `html/infrastructure/urls` that the
//! measurement ranked (docs/dev/evidence/aetherwpt/WPT.md, the AETHERWPT5 section), each from its `.py` source at
//! the pinned commit. wss/h2/https variants are still not served (no TLS server).

use gneiss_pal::api::http::ws::{accept_key, close_payload, encode_frame, FrameDecoder, OP_BINARY, OP_CLOSE, OP_CONT, OP_PING, OP_PONG, OP_TEXT};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;
use std::collections::HashMap;
use std::sync::Mutex;

/// The Python handlers this server re-implements (by path relative to the WPT root).
pub const PY_HANDLERS: &[&str] = &[
    "eventsource/resources/message.py",
    "eventsource/resources/message2.py",
    "eventsource/resources/last-event-id.py",
    "eventsource/resources/last-event-id2.py",
    "eventsource/resources/status-error.py",
    "eventsource/resources/status-reconnect.py",
    "eventsource/resources/reconnect-fail.py",
    "xhr/resources/content.py",
    "xhr/resources/status.py",
    "xhr/resources/delay.py",
    "xhr/resources/trickle.py",
    "xhr/resources/inspect-headers.py",
    "xhr/resources/headers.py",
    "xhr/resources/redirect.py",
    "xhr/resources/echo-method.py",
    "xhr/resources/requri.py",
    "xhr/resources/corsenabled.py",
    "common/redirect.py",
    // AETHERWPT2 (SR135)
    "fetch/api/resources/redirect.py",
    "fetch/api/resources/inspect-headers.py",
    "fetch/api/resources/clean-stash.py",
    "fetch/api/resources/stash-put.py",
    "fetch/api/resources/stash-take.py",
    "fetch/api/resources/status.py",
    "fetch/api/resources/method.py",
    "fetch/api/resources/echo-content.py",
    "fetch/api/resources/trickle.py",
    "fetch/api/resources/infinite-slow-response.py",
    "common/slow.py",
    "common/echo.py",
    // AETHERWPT5 (SR144)
    "html/semantics/embedded-content/media-elements/loading-the-media-resource/resources/delayed-broken-video.py",
    "html/semantics/scripting-1/the-script-element/serve-with-content-type.py",
    "html/semantics/scripting-1/the-script-element/log.py",
    "html/semantics/scripting-1/the-script-element/serve-custom-response.py",
    "html/semantics/scripting-1/the-script-element/resources/load-error-events.py",
    "html/semantics/scripting-1/the-script-element/css-module/resources/load-error-events.py",
    "html/semantics/scripting-1/the-script-element/json-module/load-error-events.py",
    "html/semantics/scripting-1/the-script-element/text-module/load-error-events.py",
    "html/semantics/scripting-1/the-script-element/module/resources/delayed-modulescript.py",
    "html/semantics/scripting-1/the-script-element/module/resources/referrer-checker.py",
    "html/semantics/scripting-1/the-script-element/resources/cross-origin.py",
    "html/semantics/scripting-1/the-script-element/script-not-found-not-executed.py",
    "html/semantics/scripting-1/the-script-element/text-module/serve-text-then-js.py",
    "html/semantics/scripting-1/the-script-element/json-module/http-accept-header-checker.py",
    "html/semantics/scripting-1/the-script-element/text-module/http-accept-header-checker.py",
    "html/semantics/scripting-1/the-script-element/css-module/resources/record-fetch.py",
    "html/semantics/embedded-content/the-iframe-element/support/download_stash.py",
    "html/semantics/embedded-content/the-img-element/404-response-with-actual-image-data.py",
    "html/semantics/embedded-content/the-img-element/resources/image-alternate-no-store.py",
    "html/semantics/embedded-content/the-img-element/resources/referrer-checker-img.py",
    "html/semantics/embedded-content/the-img-element/resources/image-and-stash.py",
    "html/browsers/browsing-the-web/navigating-across-documents/resources/redirect.py",
    "html/browsers/browsing-the-web/navigating-across-documents/refresh/resources/refresh.py",
    "html/browsers/browsing-the-web/overlapping-navigations-and-traversals/resources/slow.py",
    "html/browsers/browsing-the-web/navigating-across-documents/resources/no-cache-single-redirect.py",
    "html/browsers/browsing-the-web/history-traversal/resources/unset_context_name_stash.py",
    "html/infrastructure/urls/resolving-urls/query-encoding/resources/blank.py",
    "html/infrastructure/urls/terminology-0/resources/echo-referrer-text.py",
    "xhr/resources/echo-headers.py",
    "xhr/resources/infinite-redirects.py",
    "xhr/resources/parse-headers.py",
    "xhr/resources/json-with-bom.py",
    "xhr/resources/win-1252-xml.py",
    "xhr/resources/invalid-utf8.py",
    "xhr/resources/echo-content-type.py",
    "xhr/resources/get-set-cookie.py",
    "xhr/resources/bad-chunk-encoding.py",
    "xhr/resources/upload.py",
    "xhr/resources/chunked.py",
    "xhr/resources/last-modified.py",
    "xhr/resources/accept.py",
    "xhr/resources/conditional.py",
    "xhr/resources/form.py",
    "resource-timing/resources/fake_responses.py",
    "resource-timing/resources/empty.py",
    "resource-timing/resources/TAOResponse.py",
    "resource-timing/resources/status-code.py",
    "resource-timing/resources/redirect-without-location.py",
    "resource-timing/resources/resource-timing-content-length.py",
    "resource-timing/resources/redirect-cors.py",
    "resource-timing/resources/cacheable-and-validated.py",
    "encoding/resources/single-byte-raw.py",
    "encoding/resources/json-charset.py",
    "encoding/resources/text-plain-charset.py",
    "encoding/resources/text-html-meta-charset.py",
    "url/resources/percent-encoding.py",
    "fetch/api/resources/bad-chunk-encoding.py",
    "fetch/api/resources/script-with-header.py",
    "fetch/api/resources/cache.py",
    "fetch/api/resources/redirect-empty-location.py",
    "fetch/api/resources/stashed-chunk.py",
    "eventsource/resources/cors.py",
    "eventsource/resources/cors-cookie.py",
    "FileAPI/file/resources/echo-content-escaped.py",
    "websockets/cookies/support/set-cookie.py",
    "websockets/opening-handshake/resources/redirect_response.py",
    "websockets/security/check.py",
    // AETHERWPT5 (SR144): the CORS handlers the cross-origin lift reaches.
    "fetch/api/resources/preflight.py",
    "xhr/resources/reset-token.py",
    "xhr/resources/access-control-preflight-denied.py",
    "xhr/resources/access-control-basic-put-allow.py",
    "xhr/resources/access-control-basic-denied.py",
    "xhr/resources/access-control-basic-options-not-supported.py",
    "xhr/resources/access-control-preflight-request-invalid-status.py",
    "xhr/resources/access-control-basic-allow-star.py",
    "xhr/resources/access-control-basic-allow.py",
    "xhr/resources/access-control-origin-header.py",
    "xhr/resources/access-control-allow-with-body.py",
    "xhr/resources/access-control-basic-cors-safelisted-request-headers.py",
    "xhr/resources/access-control-basic-cors-safelisted-response-headers.py",
    "xhr/resources/access-control-preflight-request-allow-headers-returns-star.py",
    "xhr/resources/access-control-preflight-request-header-lowercase.py",
    "xhr/resources/access-control-preflight-request-header-returns-origin.py",
    "xhr/resources/access-control-preflight-request-header-sorted.py",
    "xhr/resources/access-control-preflight-request-headers-origin.py",
    "xhr/resources/access-control-cookie.py",
    "xhr/resources/echo-content-cors.py",
    "xhr/resources/redirect-cors.py",
    "xhr/resources/header-user-agent.py",
    "xhr/resources/win-1252-html.py",
    "xhr/resources/invalid-utf8-html.py",
    "resource-timing/resources/multi_redirect.py",
    "common/slow-redirect.py",
    "html/semantics/scripting-1/the-script-element/module/dynamic-import/beta/redirect.py",
];

/// The pywebsocket handlers the WebSocket server re-implements (`/name` = `websockets/handlers/name_wsh.py`).
pub const WS_HANDLERS: &[&str] = &[
    "echo",
    "echo_exit",
    "echo_raw",
    "echo_close_data",
    "empty-message",
    "echo-query",
    "echo-query_v13",
    "protocol",
    "protocol_array",
    "handshake_protocol",
    "handshake_no_protocol",
    "handshake_no_extensions",
    "simple_handshake",
    "wrong_accept_key",
    "invalid",
    "origin",
    "echo-cookie",
    "set-cookie",
    "set-cookie_http",
    "handshake_sleep_2",
    "referrer",
    "remote-close",
    "delayed-passive-close",
];

/// AETHERWPT5 (SR144): Aether's `testdriver-vendor.js` — `window.test_driver_internal`'s WebDriver commands over
/// the engine's automation queue (`js/testdriver.rs`: `__td_queue(kind, json)` returns a ticket the engine settles
/// through `__td_settle(ticket, error, result)` after dispatching the command — `AetherEngine::pump_automation`).
/// `in_automation` is true, so a command the vendor does not implement rejects at once instead of waiting for a
/// person. Element Click's point and Perform Actions' element origins are the engine's own layout
/// (`getClientRects`, CSSOM View); a point in a frame's viewport is moved to the top-level one through the
/// frame element's border box.
pub const TESTDRIVER_VENDOR_JS: &str = r#"(function () {
  if (typeof __td_queue !== 'function' || typeof __td_node !== 'function' || !window.test_driver_internal) { return; }
  var pending = {};
  function call(kind, payload) {
    return new Promise(function (resolve, reject) {
      var ticket = __td_queue(kind, JSON.stringify(payload));
      pending[ticket] = { resolve: resolve, reject: reject };
    });
  }
  window.__td_settle = function (ticket, error, result) {
    var p = pending[ticket];
    if (!p) { return; }
    delete pending[ticket];
    if (error !== null && error !== undefined) { p.reject(new Error(error)); } else { p.resolve(result === undefined ? null : result); }
  };
  function toTop(win, x, y) {
    try {
      while (win && win !== window) {
        var fe = win.frameElement;
        if (!fe) { break; }
        var r = fe.getBoundingClientRect();
        x += r.left + (fe.clientLeft || 0);
        y += r.top + (fe.clientTop || 0);
        win = win.parent;
      }
    } catch (e) {}
    return { x: x, y: y };
  }
  function winOf(el) { return el && el.ownerDocument ? el.ownerDocument.defaultView : window; }
  var td = window.test_driver_internal;
  td.in_automation = true;
  td.click = function (element, coords) {
    var p = toTop(winOf(element), coords.x, coords.y);
    return call('click', { x: p.x, y: p.y });
  };
  td.send_keys = function (element, keys) {
    try { element.focus(); } catch (e) {}
    return call('keys', { target: __td_node(element), keys: String(keys) });
  };
  td.action_sequence = function (actions, context) {
    var sources = [];
    for (var i = 0; i < actions.length; i++) {
      var src = actions[i];
      var list = [];
      var acts = src.actions || [];
      for (var j = 0; j < acts.length; j++) {
        var a = acts[j];
        var o = {};
        for (var k in a) { if (k !== 'origin') { o[k] = a[k]; } }
        if (a.origin !== undefined && a.origin !== null) {
          if (typeof a.origin === 'string') { o.origin = a.origin; }
          else {
            var id = __td_node(a.origin);
            if (id === null) { return Promise.reject(new Error('invalid argument: origin')); }
            o.origin = { node: id };
          }
        }
        list.push(o);
      }
      sources.push({ type: src.type, id: src.id, pointerType: (src.parameters && src.parameters.pointerType) || 'mouse', actions: list });
    }
    return call('actions', { sources: sources });
  };
  td.set_permission = function (params, context) {
    var d = (params && params.descriptor) || {};
    return call('permission', { name: String(d.name), state: String(params && params.state) });
  };
  td.get_all_cookies = function (context) { return call('cookies_get', {}); };
  td.get_named_cookie = function (name, context) { return call('cookie_named', { name: String(name) }); };
  td.delete_all_cookies = function (context) { return call('cookies_clear', {}); };
  td.set_test_context = function (context) {};
})();
"#;

#[derive(Clone, Copy)]
pub struct Ports {
    /// `{{ports[http][0]}}`: the page origin's port (`http://localhost:<http>`).
    pub http: u16,
    /// AETHERWPT5 (SR144): `{{ports[http][1]}}`, the same server on a second port (a different origin).
    pub http2: u16,
    pub ws: u16,
}

/// AETHERWPT5 (SR144): the host name the runner loads test pages from (`{{host}}`); its origin is
/// `http://localhost:{{ports[http][0]}}`.
pub const HOST: &str = "localhost";

/// The loopback addresses the stand-in listens on, for the `sub` pipe's other hosts: `www` / `www1` (and
/// get-host-info's REMOTE_HOST and NOTSAMESITE_HOST under a `localhost` page host), `www2`, and the `alt` hosts.
const HOSTS: &[(&str, &str)] = &[("www", "127.0.0.1"), ("www1", "127.0.0.1"), ("www2", "127.0.0.2"), ("alt", "127.0.0.3"), ("alt.www", "127.0.0.4"), ("alt.www1", "127.0.0.5"), ("alt.www2", "127.0.0.6")];

fn host_of(key: &str) -> &'static str {
    HOSTS.iter().find(|(k, _)| *k == key).map(|(_, v)| *v).unwrap_or("127.0.0.1")
}

struct Req {
    method: String,
    target: String,
    path: String,
    query: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Req {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(k, _)| k.eq_ignore_ascii_case(name)).map(|(_, v)| v.as_str())
    }
    /// wptserve's `request.GET.first(name)` (urllib's parse_qsl: `+` is a space, %XX decoded), as bytes.
    fn get(&self, name: &str) -> Option<Vec<u8>> {
        for kv in self.query.split('&') {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            if pct(k) == name.as_bytes() {
                return Some(pct(v));
            }
        }
        None
    }
    fn get_str(&self, name: &str) -> Option<String> {
        self.get(name).map(|b| String::from_utf8_lossy(&b).into_owned())
    }
    fn cookie(&self, name: &str) -> Option<String> {
        let c = self.header("cookie")?;
        c.split(';').find_map(|p| {
            let (k, v) = p.trim().split_once('=')?;
            (k == name).then(|| v.to_string())
        })
    }
    /// AETHERWPT5 (SR144): wptserve's `request.POST` — the body's fields as (name, value, filename, content type)
    /// in order: `application/x-www-form-urlencoded` (urllib's parse_qsl) or `multipart/form-data` (cgi's
    /// FieldStorage; a part with a `filename` is a file, `Content-Type` text/plain when absent).
    fn post(&self) -> Vec<(Vec<u8>, Vec<u8>, Option<String>, Option<String>)> {
        let ct = self.header("content-type").unwrap_or("").to_string();
        let mut out = Vec::new();
        if let Some(b) = ct.split(';').map(str::trim).find_map(|p| p.strip_prefix("boundary=")) {
            let b = b.trim_matches('"');
            let delim = format!("--{b}").into_bytes();
            let body = &self.body;
            let mut parts: Vec<&[u8]> = Vec::new();
            let mut i = 0;
            let mut starts = Vec::new();
            while let Some(p) = find(&body[i..], &delim) {
                starts.push(i + p);
                i += p + delim.len();
            }
            for w in starts.windows(2) {
                parts.push(&body[w[0] + delim.len()..w[1]]);
            }
            for part in parts {
                let part = part.strip_prefix(b"\r\n").unwrap_or(part);
                let Some(h) = find(part, b"\r\n\r\n") else { continue };
                let head = String::from_utf8_lossy(&part[..h]).into_owned();
                let mut value = &part[h + 4..];
                if value.ends_with(b"\r\n") {
                    value = &value[..value.len() - 2];
                }
                let mut name = Vec::new();
                let mut filename = None;
                let mut ptype = None;
                for line in head.lines() {
                    let Some((k, v)) = line.split_once(':') else { continue };
                    if k.trim().eq_ignore_ascii_case("content-disposition") {
                        for param in v.split(';').map(str::trim) {
                            if let Some(n) = param.strip_prefix("name=") {
                                name = n.trim_matches('"').as_bytes().to_vec();
                            } else if let Some(f) = param.strip_prefix("filename=") {
                                filename = Some(f.trim_matches('"').to_string());
                            }
                        }
                    } else if k.trim().eq_ignore_ascii_case("content-type") {
                        ptype = Some(v.trim().to_string());
                    }
                }
                if filename.is_some() && ptype.is_none() {
                    ptype = Some("text/plain".into());
                }
                out.push((name, value.to_vec(), filename, ptype));
            }
        } else {
            for kv in String::from_utf8_lossy(&self.body).split('&').filter(|x| !x.is_empty()) {
                let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
                out.push((pct(k), pct(v), None, None));
            }
        }
        out
    }
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// A base64 decoder (RFC 4648 §4, padding optional) for the handlers that carry base64 (`decodebytes`,
/// `b64decode`).
fn b64decode(s: &str) -> Vec<u8> {
    let mut out = Vec::new();
    let mut acc: u32 = 0;
    let mut bits = 0;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => continue,
        } as u32;
        acc = (acc << 6) | v;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    out
}

/// `Set-Cookie` as wptserve's `response.set_cookie(name, value, expires=…, secure=…)` writes it (http.cookies'
/// Morsel: `name=value; expires=<date>; Path=/; Secure`).
fn set_cookie_header(name: &str, value: &str, expires: Option<&str>, secure: bool) -> String {
    let mut h = format!("{name}={value}");
    if let Some(e) = expires {
        h.push_str(&format!("; expires={e}"));
    }
    h.push_str("; Path=/");
    if secure {
        h.push_str("; Secure");
    }
    h
}

/// An HTTP-date (RFC 7231 §7.1.1.1 IMF-fixdate) for a Unix time.
fn http_date(secs: i64) -> String {
    let days = secs.div_euclid(86400);
    let rem = secs.rem_euclid(86400);
    let (h, m, sec) = (rem / 3600, rem % 3600 / 60, rem % 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z.rem_euclid(146097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    let wd = ["Thu", "Fri", "Sat", "Sun", "Mon", "Tue", "Wed"][days.rem_euclid(7) as usize];
    let mn = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"][(mo - 1) as usize];
    format!("{wd}, {d:02} {mn} {y} {h:02}:{m:02}:{sec:02} GMT")
}

fn pct(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 3 <= b.len() => match (hexval(b[i + 1]), hexval(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push(h << 4 | l);
                    i += 3;
                    continue;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    out
}

fn hexval(c: u8) -> Option<u8> {
    (c as char).to_digit(16).map(|d| d as u8)
}

fn read_req(stream: &mut TcpStream) -> Option<Req> {
    let mut r = BufReader::new(stream.try_clone().ok()?);
    let mut line = String::new();
    r.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        if r.read_line(&mut h).ok()? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.trim_end().split_once(':') {
            headers.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    let len: usize = headers.iter().find(|(k, _)| k.eq_ignore_ascii_case("content-length")).and_then(|(_, v)| v.parse().ok()).unwrap_or(0);
    let mut body = vec![0u8; len];
    if len > 0 {
        r.read_exact(&mut body).ok()?;
    }
    let (p, q) = target.split_once('?').unwrap_or((&target, ""));
    let path = String::from_utf8_lossy(&pct(&p.replace('+', "%2B"))).into_owned();
    Some(Req { method, path, query: q.split('#').next().unwrap_or("").to_string(), target: target.clone(), headers, body })
}

struct Resp {
    status: u16,
    reason: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl Resp {
    fn new(status: u16, reason: &str, ty: &str, body: impl Into<Vec<u8>>) -> Resp {
        let mut headers = Vec::new();
        if !ty.is_empty() {
            headers.push(("Content-Type".to_string(), ty.to_string()));
        }
        Resp { status, reason: reason.into(), headers, body: body.into() }
    }
    fn header(mut self, k: &str, v: &str) -> Resp {
        self.headers.push((k.into(), v.into()));
        self
    }
}

fn write_resp(s: &mut TcpStream, r: Resp) {
    let mut h = format!("HTTP/1.1 {} {}\r\n", r.status, r.reason);
    let has_len = r.headers.iter().any(|(k, _)| k.eq_ignore_ascii_case("content-length"));
    for (k, v) in &r.headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    if !has_len {
        h.push_str(&format!("Content-Length: {}\r\n", r.body.len()));
    }
    h.push_str("Connection: close\r\n\r\n");
    let _ = s.write_all(h.as_bytes());
    let _ = s.write_all(&r.body);
}

fn content_type(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css",
        Some("json") => "application/json",
        Some("xml") => "application/xml",
        Some("svg") => "image/svg+xml",
        Some("xhtml") => "application/xhtml+xml",
        Some("txt") => "text/plain",
        Some("event_stream") => "text/event-stream",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        _ => "application/octet-stream",
    }
}

/// wptserve's `sub` pipe for the keys these directories use.
fn subst(text: &str, req: &Req, ports: Ports) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find("{{") {
        out.push_str(&rest[..i]);
        let Some(j) = rest[i..].find("}}") else {
            out.push_str(&rest[i..]);
            return out;
        };
        let key = rest[i + 2..i + j].trim();
        let v = match key {
            // AETHERWPT5 (SR144): the page host is `localhost` (wptserve's localhost configuration); the other
            // hosts are the loopback addresses the stand-in listens on (HOSTS); a `nonexistent` host does not resolve.
            "host" | "domains[]" | "hosts[][]" | "location[hostname]" => Some(HOST.to_string()),
            "hosts[][nonexistent]" | "hosts[alt][nonexistent]" => Some("nonexistent.invalid".to_string()),
            k if k.starts_with("domains[") => Some(host_of(k.trim_start_matches("domains[").trim_end_matches(']')).to_string()),
            k if k.starts_with("hosts[") => {
                let inner = k.trim_start_matches("hosts[").trim_end_matches(']');
                let (ns, sub) = inner.split_once("][").unwrap_or((inner, ""));
                Some(match (ns, sub) {
                    ("", "") => HOST.to_string(),
                    ("", sub) => host_of(sub).to_string(),
                    ("alt", "") => host_of("alt").to_string(),
                    ("alt", sub) => host_of(&format!("alt.{sub}")).to_string(),
                    _ => host_of("alt").to_string(),
                })
            }
            "ports[http][0]" => Some(ports.http.to_string()),
            "ports[http][1]" => Some(ports.http2.to_string()),
            "ports[ws][0]" => Some(ports.ws.to_string()),
            // No TLS server: an unused port, so wss/https variants fail fast (and are skipped by name).
            "ports[https][0]" | "ports[wss][0]" | "ports[h2][0]" | "ports[https][1]" => Some("1".into()),
            "location[scheme]" => Some("http".into()),
            "location[host]" | "location[server]" => Some(req.header("host").map(str::to_string).unwrap_or_else(|| format!("{HOST}:{}", ports.http))),
            "location[port]" => Some(req.header("host").and_then(|h| h.rsplit_once(':').map(|(_, p)| p.to_string())).unwrap_or_else(|| ports.http.to_string())),
            "location[path]" | "location[pathname]" => Some(req.path.clone()),
            "location[query]" => Some(if req.query.is_empty() { String::new() } else { format!("?{}", req.query) }),
            "uuid()" => Some(format!("{:016x}-0000-4000-8000-000000000000", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64)),
            k => {
                if let Some(h) = k.strip_prefix("headers[").and_then(|x| x.strip_suffix(']')) {
                    Some(req.header(h.trim_matches('"')).unwrap_or("").to_string())
                } else if let Some(g) = k.strip_prefix("GET[").and_then(|x| x.strip_suffix(']')) {
                    Some(req.get_str(g.trim_matches('"')).unwrap_or_default())
                } else if let Some(a) = k.strip_prefix("header_or_default(").and_then(|x| x.strip_suffix(')')) {
                    // AETHERWPT5 (SR144): wptserve's `header_or_default(name, default)`.
                    let (h, d) = a.split_once(',').unwrap_or((a, ""));
                    Some(req.header(h.trim()).unwrap_or(d.trim()).to_string())
                } else {
                    None
                }
            }
        };
        match v {
            Some(v) => out.push_str(&v),
            None => out.push_str(&rest[i..i + j + 2]),
        }
        rest = &rest[i + j + 2..];
    }
    out.push_str(rest);
    out
}

fn wrapper(root: &Path, stem: &str, kind: &str, report_js: &str) -> Option<Vec<u8>> {
    let _ = report_js;
    let src = std::fs::read_to_string(root.join(format!("{stem}.{kind}.js"))).ok()?;
    let mut metas = String::new();
    let mut title = String::new();
    for l in src.lines() {
        if let Some(sc) = l.strip_prefix("// META: script=") {
            metas.push_str(&format!("<script src=\"{}\"></script>\n", sc.trim()));
        } else if let Some(t) = l.strip_prefix("// META: title=") {
            title = format!("<title>{}</title>\n", t.trim());
        }
    }
    let name = Path::new(stem).file_name()?.to_string_lossy().to_string();
    Some(
        format!(
            "<!doctype html>\n<meta charset=utf-8>\n{title}<script>self.GLOBAL = {{ isWindow: function () {{ return true; }}, isWorker: function () {{ return false; }}, isShadowRealm: function () {{ return false; }} }};</script>\n<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n{metas}<div id=log></div>\n<script src=\"{name}.{kind}.js\"></script>\n"
        )
        .into_bytes(),
    )
}

/// The `// META:` lines of a test source: (`<script src>` / `importScripts` URLs, the title).
fn metas(src: &str) -> (Vec<String>, String) {
    let mut scripts = Vec::new();
    let mut title = String::new();
    for l in src.lines() {
        if let Some(sc) = l.strip_prefix("// META: script=") {
            scripts.push(sc.trim().to_string());
        } else if let Some(t) = l.strip_prefix("// META: title=") {
            title = t.trim().to_string();
        }
    }
    (scripts, title)
}

/// AETHERWORKERS3 (SR129): wptserve's `WorkersHandler` — the `.worker.html` page of a `X.worker.js` test (and
/// the `.any.worker.html` page of a `X.any.js` test): testharness in the window, the test itself in a dedicated
/// worker whose results `fetch_tests_from_worker` (testharness.js's worker shim: a MessageChannel the worker's
/// harness reports through) brings back. `script`: the worker script's suffix (`worker.js` / `any.worker.js`);
/// `source`: the test file's (`worker.js` / `any.js`), which must exist.
fn worker_wrapper(root: &Path, stem: &str, script: &str, source: &str) -> Option<Vec<u8>> {
    let src = std::fs::read_to_string(root.join(format!("{stem}.{source}"))).ok()?;
    let (_, title) = metas(&src);
    let title = if title.is_empty() { String::new() } else { format!("<title>{title}</title>\n") };
    let name = Path::new(stem).file_name()?.to_string_lossy().to_string();
    Some(
        format!(
            "<!doctype html>\n<meta charset=utf-8>\n{title}<script src=\"/resources/testharness.js\"></script>\n<script src=\"/resources/testharnessreport.js\"></script>\n<div id=log></div>\n<script>\nfetch_tests_from_worker(new Worker(\"{name}.{script}\" + location.search));\n</script>\n"
        )
        .into_bytes(),
    )
}

/// AETHERWORKERS3 (SR129): wptserve's `ClassicWorkerHandler` — the generated `X.any.worker.js`: the worker
/// global's `GLOBAL`, testharness, each `// META: script=` through `importScripts`, the `.any.js` test, `done()`.
fn any_worker_script(root: &Path, stem: &str) -> Option<Vec<u8>> {
    let src = std::fs::read_to_string(root.join(format!("{stem}.any.js"))).ok()?;
    let (scripts, _) = metas(&src);
    let imports: String = scripts.iter().map(|u| format!("importScripts({u:?});\n")).collect();
    Some(
        format!(
            "self.GLOBAL = {{ isWindow: function () {{ return false; }}, isWorker: function () {{ return true; }}, isShadowRealm: function () {{ return false; }} }};\nimportScripts(\"/resources/testharness.js\");\n{imports}importScripts(\"/{stem}.any.js\");\ndone();\n"
        )
        .into_bytes(),
    )
}

// ------------------------------------------------------------------------------------------------ stash

/// A value in wptserve's stash: a string (`stash-put.py`) or `redirect.py`'s dictionary.
#[derive(Clone)]
enum Stashed {
    Str(String),
    Redirect { count: u32, preflight: bool },
    /// AETHERWPT5 (SR144): `serve-custom-response.py`'s context.
    Custom { network_error: bool, run_count: u32, status_code: u16, content_type: String },
    /// AETHERWPT5 (SR144): `fetch/api/resources/preflight.py`'s stashed dictionary.
    Dict(Vec<(String, String)>),
}

/// `stash.take(key)` as a string, or None.
fn stash_take_str(key: &str, dir: &str) -> Option<String> {
    match stash_take(key, dir) {
        Some(Stashed::Str(s)) => Some(s),
        _ => None,
    }
}

/// `stash.take(key)` as a counter (0 when absent), as the handlers that count requests read it.
fn stash_count(key: &str, dir: &str) -> u32 {
    stash_take_str(key, dir).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// wptserve's stash: (key, URL directory) → value. Process-wide, as wptserve's is server-wide.
static STASH: Mutex<Option<HashMap<(String, String), Stashed>>> = Mutex::new(None);

fn stash_take(key: &str, dir: &str) -> Option<Stashed> {
    STASH.lock().unwrap().get_or_insert_with(HashMap::new).remove(&(key.to_string(), dir.to_string()))
}

/// `stash.put`: false when the key is already present (wptserve raises StashError).
fn stash_put(key: &str, dir: &str, v: Stashed) -> bool {
    let mut g = STASH.lock().unwrap();
    let m = g.get_or_insert_with(HashMap::new);
    let k = (key.to_string(), dir.to_string());
    if m.contains_key(&k) {
        return false;
    }
    m.insert(k, v);
    true
}

/// `request.url_parts.path`'s directory with its trailing `/` (the stash's default path).
fn url_dir(req: &Req) -> String {
    match req.path.rfind('/') {
        Some(i) => req.path[..=i].to_string(),
        None => "/".to_string(),
    }
}

/// Bytes as Python's `isomorphic_decode` sees them (Latin-1), then `urllib.parse.quote_plus` of that `str`
/// (UTF-8, `+` for a space, `_.-~` and alphanumerics kept).
fn quote_plus(b: &[u8]) -> String {
    let s: String = b.iter().map(|&c| c as char).collect();
    let mut out = String::new();
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_' | b'.' | b'-' | b'~' => out.push(*byte as char),
            b' ' => out.push('+'),
            c => out.push_str(&format!("%{c:02X}")),
        }
    }
    out
}

/// `urlparse(url).scheme`: the leading `[A-Za-z][A-Za-z0-9+.-]*` before a `:`, lowercased, or "".
fn url_scheme(u: &str) -> String {
    match u.find(':') {
        Some(i) if i > 0 && u.as_bytes()[0].is_ascii_alphabetic() && u[..i].bytes().all(|c| c.is_ascii_alphanumeric() || b"+.-".contains(&c)) => u[..i].to_ascii_lowercase(),
        _ => String::new(),
    }
}

/// The query's (name, first value) pairs in first-seen order (Python's `dict` over `request.GET.items()`).
fn query_pairs(query: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut out: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for kv in query.split('&').filter(|x| !x.is_empty()) {
        let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
        let (k, v) = (pct(k), pct(v));
        if !out.iter().any(|(x, _)| *x == k) {
            out.push((k, v));
        }
    }
    out
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// The emulated Python handlers. `None` = not emulated.
fn py(rel: &str, req: &Req, s: &mut TcpStream, root: &Path, ports: Ports) -> Option<Option<Resp>> {
    let es = "text/event-stream";
    let dir = url_dir(req);
    // The handler's own directory in the checkout (`os.path.dirname(__file__)`).
    let here = root.join(rel).parent().map(Path::to_path_buf).unwrap_or_else(|| root.to_path_buf());
    let read = |p: &Path| std::fs::read(p).unwrap_or_default();
    let referer = req.header("referer").unwrap_or("").to_string();
    let origin = req.header("origin").map(str::to_string);
    // wptserve's `response.set_error(code, message)`: a text/plain body "<code> <reason>\n\n<message>"-shaped error.
    let set_error = |code: u16, msg: &str| Resp::new(code, "Error", "text/plain", msg.as_bytes().to_vec());
    Some(Some(match rel {
        "eventsource/resources/message.py" => {
            let mime = req.get_str("mime").unwrap_or_else(|| es.into());
            let mut body = req.get("message").unwrap_or_else(|| b"data: data".to_vec());
            if req.get("newline").as_deref() != Some(b"none") {
                body.extend_from_slice(b"\n\n");
            }
            body.push(b'\n');
            if let Some(ms) = req.get_str("sleep").and_then(|v| v.parse::<u64>().ok()) {
                sleep_ms(ms);
            }
            Resp::new(200, "OK", &mime, body)
        }
        "eventsource/resources/message2.py" => {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n");
            for _ in 0..10 {
                let chunk = b"data:msg\ndata: msg\n\n:\nfalsefield:msg\n\nfalsefield:msg\nData:data\n\ndata\n\ndata:end\n\n";
                if s.write_all(chunk).is_err() {
                    break;
                }
                sleep_ms(2000);
            }
            return Some(None);
        }
        "eventsource/resources/last-event-id.py" => {
            let body = match req.header("last-event-id") {
                Some(id) if !id.is_empty() => format!("data: {id}\n\n").into_bytes(),
                _ => {
                    let mut b = b"id: ".to_vec();
                    b.extend(req.get("idvalue").unwrap_or_else(|| "\u{2026}".as_bytes().to_vec()));
                    b.extend_from_slice(b"\nretry: 200\ndata: hello\n\n");
                    b
                }
            };
            Resp::new(200, "OK", es, body)
        }
        "eventsource/resources/last-event-id2.py" => {
            let ty = req.get_str("type").and_then(|t| t.parse::<u32>().ok()).unwrap_or(1);
            let body: &[u8] = match ty {
                1 => b"id: 1\ndata: 1\n\ndata: 2\n\nid: 2\ndata:3\n\ndata:4\n\n",
                2 => b"id: 1\ndata: 1\n\nid:\ndata:2\n\ndata:3\n\n",
                3 => b"id: 1\ndata: 1\n\nid\ndata:2\n\ndata:3\n\n",
                _ => b"data: invalid_test\n\n",
            };
            Resp::new(200, "OK", es, body)
        }
        "eventsource/resources/status-error.py" => {
            let code: u16 = req.get_str("status").and_then(|v| v.parse().ok()).unwrap_or(404);
            let body: &[u8] = if code == 204 || code == 205 { b"" } else { b"data: data\n\n" };
            Resp::new(code, "HAHAHAHA", es, body)
        }
        "eventsource/resources/status-reconnect.py" => {
            let status = req.get_str("status").unwrap_or_else(|| "204".into());
            let name = req.get_str("id").unwrap_or_else(|| status.clone());
            let cookie = format!("request{name}");
            if req.cookie(&cookie).as_deref() == Some(status.as_str()) {
                Resp::new(200, "OK", es, b"data: data\n\n".to_vec()).header("Set-Cookie", &format!("{cookie}=; Max-Age=0; Path=/"))
            } else {
                let mut body = b"retry: 2\n".to_vec();
                if req.get("ok_first").is_some() {
                    body.extend_from_slice(b"data: ok\n\n");
                }
                Resp::new(status.parse().unwrap_or(204), "TEST", es, body).header("Set-Cookie", &format!("{cookie}={status}; Path=/"))
            }
        }
        "eventsource/resources/reconnect-fail.py" => {
            let name = format!("recon_fail_{}", req.get_str("id").unwrap_or_default());
            match req.cookie(&name).as_deref() {
                Some("opened") => Resp::new(200, "RECONNECT", es, b"data: reconnected\n\n".to_vec()).header("Set-Cookie", &format!("{name}=reconnected; Path=/")),
                Some("reconnected") => Resp::new(204, "NO CONTENT (CLOSE)", es, Vec::new()).header("Set-Cookie", &format!("{name}=; Max-Age=0; Path=/")),
                _ => Resp::new(200, "OPEN", es, b"retry: 2\ndata: opened\n\n".to_vec()).header("Set-Cookie", &format!("{name}=opened; Path=/")),
            }
        }
        "xhr/resources/content.py" => {
            let mut ty = "text/plain".to_string();
            if let Some(cs) = req.get_str("response_charset_label") {
                ty.push_str(&format!(";charset={cs}"));
            }
            let content = req.get("content").unwrap_or_else(|| req.body.clone());
            Resp::new(200, "OK", &ty, content)
                .header("X-Request-Method", &req.method)
                .header("X-Request-Query", if req.query.is_empty() { "NO" } else { &req.query })
                .header("X-Request-Content-Length", req.header("content-length").unwrap_or("NO"))
                .header("X-Request-Content-Type", req.header("content-type").unwrap_or("NO"))
        }
        "xhr/resources/status.py" => {
            let code: u16 = req.get_str("code").and_then(|v| v.parse().ok()).unwrap_or(200);
            let text = req.get_str("text").unwrap_or_else(|| "OMG".into());
            let content = req.get("content").unwrap_or_default();
            let ty = req.get_str("type").unwrap_or_default();
            let mut r = Resp::new(code, &text, "", content);
            r.headers.push(("Content-Type".into(), ty));
            r.header("X-Request-Method", &req.method)
        }
        "xhr/resources/delay.py" => {
            let ms: f64 = req.get_str("ms").and_then(|v| v.parse().ok()).unwrap_or(500.0);
            sleep_ms(ms as u64);
            Resp::new(200, "OK", "text/plain", b"TEST_DELAY".to_vec())
                .header("Access-Control-Allow-Origin", "*")
                .header("Access-Control-Allow-Methods", "YO")
        }
        "xhr/resources/trickle.py" => {
            let chunk = b"TEST_TRICKLE\n";
            let delay: f64 = req.get_str("ms").and_then(|v| v.parse().ok()).unwrap_or(500.0);
            let count: usize = req.get_str("count").and_then(|v| v.parse().ok()).unwrap_or(50);
            sleep_ms(delay as u64);
            let mut head = String::from("HTTP/1.1 200 OK\r\nContent-type: text/plain\r\nConnection: close\r\n");
            if req.get("specifylength").is_some() {
                head.push_str(&format!("Content-Length: {}\r\n", count * chunk.len()));
            }
            head.push_str("\r\n");
            let _ = s.write_all(head.as_bytes());
            sleep_ms(delay as u64);
            for _ in 0..count {
                if s.write_all(chunk).is_err() {
                    break;
                }
                sleep_ms(delay as u64);
            }
            return Some(None);
        }
        "xhr/resources/inspect-headers.py" => {
            let filter_value = req.get_str("filter_value").unwrap_or_default();
            let filter_name = req.get_str("filter_name").unwrap_or_default().to_ascii_lowercase();
            let mut result = String::new();
            for (k, v) in &req.headers {
                if !filter_value.is_empty() {
                    if *v == filter_value {
                        result.push_str(&format!("{k},"));
                    }
                } else if k.to_ascii_lowercase() == filter_name {
                    result.push_str(&format!("{k}: {v}\n"));
                }
            }
            let mut r = Resp::new(200, "OK", "text/plain", result.into_bytes());
            if req.get("cors").is_some() {
                r = r.header("Access-Control-Allow-Origin", "*").header("Access-Control-Allow-Credentials", "true");
            }
            r
        }
        "xhr/resources/headers.py" => Resp::new(200, "OK", "text/plain", b"TEST".to_vec())
            .header("X-Custom-Header", "test")
            .header("Set-Cookie", "test")
            .header("Set-Cookie2", "test")
            .header("X-Custom-Header-Empty", "")
            .header("X-Custom-Header-Comma", "1")
            .header("X-Custom-Header-Comma", "2")
            .header("X-Custom-Header-Bytes", "\u{2026}"),
        "xhr/resources/redirect.py" => {
            let code: u16 = req.get_str("code").and_then(|v| v.parse().ok()).unwrap_or(302);
            if let Some(d) = req.get_str("delay").and_then(|v| v.parse::<f64>().ok()) {
                sleep_ms(d as u64);
            }
            if req.get("followed").is_some() {
                Resp::new(200, "OK", "", b"MAGIC HAPPENED".to_vec()).header("Content:Type", "text/plain")
            } else {
                let mut loc = req.get_str("location").unwrap_or_else(|| format!("{}?followed", req.path));
                if loc.starts_with("redirect.py") {
                    loc.push_str(&format!("&code={code}"));
                }
                Resp::new(code, "WEBSRT MARKETING", "", b"TEST".to_vec()).header("Location", &loc)
            }
        }
        "xhr/resources/echo-method.py" => Resp::new(200, "OK", "text/plain", req.method.clone().into_bytes()),
        "xhr/resources/requri.py" => {
            // `request.url`: the scheme, the Host header and the request target.
            let body = if req.get("full").is_some() { format!("http://{}{}", req.header("host").unwrap_or(HOST), req.target) } else { req.path.clone() };
            Resp::new(200, "OK", "text/html", body.into_bytes())
        }
        "xhr/resources/corsenabled.py" => {
            if let Some(d) = req.get_str("delay").and_then(|v| v.parse::<u64>().ok()) {
                sleep_ms(d * 1000);
            }
            Resp::new(200, "OK", "", b"Test".to_vec())
                .header("Access-Control-Allow-Origin", "*")
                .header("X-Request-Method", &req.method)
                .header("X-Request-Query", if req.query.is_empty() { "NO" } else { &req.query })
                .header("X-Request-Content-Length", req.header("content-length").unwrap_or("NO"))
                .header("X-Request-Content-Type", req.header("content-type").unwrap_or("NO"))
                .header("X-Request-Data", &String::from_utf8_lossy(&req.body))
        }
        "common/redirect.py" => {
            let status: u16 = req.get_str("status").and_then(|v| v.parse().ok()).unwrap_or(302);
            let loc = req.get_str("location").unwrap_or_default();
            Resp::new(status, "Redirect", "", Vec::new()).header("Location", &loc)
        }
        // ---- AETHERWPT2 (SR135): fetch/api/resources/*.py and common/{slow,echo}.py, from their sources.
        "fetch/api/resources/redirect.py" => {
            let dir = url_dir(req);
            let mut count = 0u32;
            let mut preflight = false;
            let mut headers: Vec<(String, String)> = vec![("Content-Type".into(), "text/plain".into()), ("Cache-Control".into(), "no-cache".into()), ("Pragma".into(), "no-cache".into())];
            match req.header("origin") {
                Some(o) => {
                    headers.push(("Access-Control-Allow-Origin".into(), o.to_string()));
                    headers.push(("Access-Control-Allow-Credentials".into(), "true".into()));
                }
                None => headers.push(("Access-Control-Allow-Origin".into(), "*".into())),
            }
            let token = req.get_str("token");
            if let Some(t) = &token {
                if let Some(Stashed::Redirect { count: c, preflight: p }) = stash_take(t, &dir) {
                    count = c;
                    preflight = p;
                }
            }
            let resp = |status: u16, headers: Vec<(String, String)>, body: Vec<u8>| Resp { status, reason: if status == 200 { "OK".into() } else { "Redirect".into() }, headers, body };
            if req.method == "OPTIONS" {
                if let Some(h) = req.get_str("allow_headers") {
                    headers.push(("Access-Control-Allow-Headers".into(), h));
                }
                preflight = true;
                if req.get("redirect_preflight").is_none() {
                    if let Some(t) = &token {
                        stash_put(t, &dir, Stashed::Redirect { count, preflight });
                    }
                    return Some(Some(resp(200, headers, Vec::new())));
                }
            }
            let form = String::from_utf8_lossy(&req.body).into_owned();
            let status: u16 = req
                .get_str("redirect_status")
                .or_else(|| query_pairs(&form).into_iter().find(|(k, _)| k == b"redirect_status").map(|(_, v)| String::from_utf8_lossy(&v).into_owned()))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(302);
            count += 1;
            if let Some(loc) = req.get("location") {
                let mut url: String = loc.iter().map(|&c| c as char).collect();
                if req.get("simple").is_none() && matches!(url_scheme(&url).as_str(), "" | "http" | "https") {
                    url.push(if url.contains('?') { '&' } else { '?' });
                    let enc: Vec<String> = query_pairs(&req.query).iter().map(|(k, v)| format!("{}={}", quote_plus(k), quote_plus(v))).collect();
                    url.push_str(&enc.join("&"));
                    url.push_str(&format!("&count={count}"));
                }
                headers.push(("Location".into(), url));
            }
            if let Some(p) = req.get_str("redirect_referrerpolicy") {
                headers.push(("Referrer-Policy".into(), p));
            }
            if let Some(d) = req.get_str("delay").and_then(|v| v.parse::<f64>().ok()) {
                sleep_ms(d as u64);
            }
            if let Some(t) = &token {
                stash_put(t, &dir, Stashed::Redirect { count, preflight });
                if let Some(max) = req.get_str("max_count").and_then(|v| v.parse::<u32>().ok()) {
                    if count > max {
                        return Some(Some(Resp::new(200, "OK", "text/html", (count - 1).to_string().into_bytes())));
                    }
                }
            }
            resp(status, headers, Vec::new())
        }
        "fetch/api/resources/inspect-headers.py" => {
            let mut r = Resp::new(200, "OK", "", Vec::new());
            let checked: Vec<String> = req.get_str("headers").map(|h| h.split('|').map(str::to_string).collect()).unwrap_or_default();
            for h in &checked {
                if let Some(v) = req.header(h) {
                    r = r.header(&format!("x-request-{h}"), v);
                }
            }
            if req.get("cors").is_some() {
                r = r.header("Access-Control-Allow-Origin", req.header("origin").unwrap_or("*"));
                r = r.header("Access-Control-Allow-Credentials", "true").header("Access-Control-Allow-Methods", "GET, POST, HEAD");
                let exposed: Vec<String> = checked.iter().map(|h| format!("x-request-{h}")).collect();
                r = r.header("Access-Control-Expose-Headers", &exposed.join(", "));
                let allow = req.get_str("allow_headers").unwrap_or_else(|| req.headers.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>().join(", "));
                r = r.header("Access-Control-Allow-Headers", &allow);
            }
            r.header("content-type", "text/plain")
        }
        "fetch/api/resources/clean-stash.py" => {
            let token = req.get_str("token").unwrap_or_default();
            let hit = stash_take(&token, &url_dir(req)).is_some();
            Resp::new(200, "OK", "text/html", if hit { b"1".to_vec() } else { b"0".to_vec() })
        }
        "fetch/api/resources/stash-put.py" => {
            if req.method == "OPTIONS" {
                return Some(Some(
                    Resp::new(200, "OK", "text/html", b"done".to_vec())
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Access-Control-Allow-Methods", "*")
                        .header("Access-Control-Allow-Headers", "*"),
                ));
            }
            let mut r = Resp::new(200, "OK", "text/html", b"done".to_vec());
            if req.get("disallow_cross_origin").is_none() {
                r = r.header("Access-Control-Allow-Origin", "*");
            } else {
                let host_origin = format!("http://{}", req.header("host").unwrap_or(""));
                let same = req.get("mode").as_deref() == Some(b"no-cors") || req.get_str("frame_origin").as_deref() == Some(host_origin.as_str());
                if !same {
                    return Some(Some(Resp::new(200, "OK", "text/html", b"not stashing for cors request".to_vec())));
                }
            }
            let key = req.get_str("key").unwrap_or_default();
            let value: String = req.get("value").unwrap_or_default().iter().map(|&c| c as char).collect();
            if !stash_put(&key, &url_dir(req), Stashed::Str(value)) {
                return Some(Some(Resp::new(500, "Internal Server Error", "text/plain", b"StashError: key already exists".to_vec())));
            }
            r
        }
        "fetch/api/resources/stash-take.py" => {
            let key = req.get_str("key").unwrap_or_default();
            let body = match stash_take(&key, &url_dir(req)) {
                Some(Stashed::Str(v)) => serde_json::to_string(&v).unwrap_or_else(|_| "null".into()),
                Some(Stashed::Redirect { count, preflight }) => format!("{{\"count\": {count}, \"preflight\": \"{}\"}}", if preflight { 1 } else { 0 }),
                // AETHERWPT5 (SR144): preflight.py's dictionary as JSON; serve-custom-response's context is never taken here.
                Some(Stashed::Dict(d)) => serde_json::Value::Object(d.into_iter().map(|(k, v)| (k, serde_json::Value::String(v))).collect()).to_string(),
                Some(Stashed::Custom { .. }) | None => "null".into(),
            };
            Resp::new(200, "OK", "application/json", body.into_bytes()).header("Access-Control-Allow-Origin", "*")
        }
        "fetch/api/resources/status.py" => {
            let code: u16 = req.get_str("code").and_then(|v| v.parse().ok()).unwrap_or(200);
            let text = req.get_str("text").unwrap_or_else(|| "OMG".into());
            let mut r = Resp::new(code, &text, "", req.get("content").unwrap_or_default());
            r.headers.push(("Content-Type".into(), req.get_str("type").unwrap_or_default()));
            r.header("X-Request-Method", &req.method)
        }
        "fetch/api/resources/method.py" => {
            let mut r = Resp::new(200, "OK", "", req.body.clone());
            if req.get("cors").is_some() {
                r = r
                    .header("Access-Control-Allow-Origin", "*")
                    .header("Access-Control-Allow-Credentials", "true")
                    .header("Access-Control-Allow-Methods", "GET, POST, PUT, FOO")
                    .header("Access-Control-Allow-Headers", "x-test, x-foo")
                    .header("Access-Control-Expose-Headers", "x-request-method");
            }
            r = r.header("x-request-method", &req.method);
            for (h, name) in [("content-type", "Content-Type"), ("content-length", "Content-Length"), ("content-encoding", "Content-Encoding"), ("content-language", "Content-Language"), ("content-location", "Content-Location")] {
                r = r.header(&format!("x-request-{h}"), req.header(name).unwrap_or("NO"));
            }
            r
        }
        "fetch/api/resources/echo-content.py" => Resp::new(200, "OK", "text/plain", req.body.clone())
            .header("X-Request-Method", &req.method)
            .header("X-Request-Content-Length", req.header("content-length").unwrap_or("NO"))
            .header("X-Request-Content-Type", req.header("content-type").unwrap_or("NO")),
        "fetch/api/resources/trickle.py" => {
            let delay: f64 = req.get_str("ms").and_then(|v| v.parse().ok()).unwrap_or(500.0);
            let count: usize = req.get_str("count").and_then(|v| v.parse().ok()).unwrap_or(50);
            sleep_ms(delay as u64);
            let mut head = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n");
            if req.get("notype").is_none() {
                head.push_str("Content-type: text/plain\r\n");
            }
            head.push_str("\r\n");
            let _ = s.write_all(head.as_bytes());
            sleep_ms(delay as u64);
            for _ in 0..count {
                if s.write_all(b"TEST_TRICKLE\n").is_err() {
                    break;
                }
                sleep_ms(delay as u64);
            }
            return Some(None);
        }
        "fetch/api/resources/infinite-slow-response.py" => {
            let dir = url_dir(req);
            let state_key = req.get_str("stateKey").unwrap_or_default();
            let abort_key = req.get_str("abortKey").unwrap_or_default();
            let write_state = |v: &str| {
                if !state_key.is_empty() {
                    stash_take(&state_key, &dir);
                    stash_put(&state_key, &dir, Stashed::Str(v.into()));
                }
            };
            write_state("open");
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-type: text/plain\r\nConnection: close\r\n\r\n");
            let mut ok = s.write_all(&[b'.'; 2048]).is_ok();
            // wptserve writes until the client goes away; the stand-in also stops after a minute so an
            // abandoned page cannot hold a thread for the rest of the run.
            let began = std::time::Instant::now();
            while ok && began.elapsed().as_secs() < 60 {
                ok = s.write_all(b".").is_ok() && s.flush().is_ok();
                if !abort_key.is_empty() && stash_take(&abort_key, &dir).is_some() {
                    break;
                }
                sleep_ms(10);
            }
            write_state("closed");
            return Some(None);
        }
        "common/slow.py" => {
            let ms: f64 = req.get_str("delay").and_then(|v| v.parse().ok()).unwrap_or(2000.0);
            sleep_ms(ms as u64);
            Resp::new(200, "OK", "", Vec::new())
        }
        "common/echo.py" => Resp::new(200, "OK", "text/html", req.get("content").unwrap_or_default()).header("X-XSS-Protection", "0"),
        // ---- AETHERWPT5 (SR144): the handlers the measurement ranked, each from its source at da1f6d20.
        "html/semantics/embedded-content/media-elements/loading-the-media-resource/resources/delayed-broken-video.py" => {
            sleep_ms(100);
            Resp::new(200, "OK", "text/plain", b"FAIL".to_vec())
        }
        "html/semantics/scripting-1/the-script-element/serve-with-content-type.py" => {
            match (req.get_str("fn"), req.get_str("ct")) {
                (Some(f), Some(ct)) if !f.contains("..") && here.join(&f).is_file() => Resp::new(200, "OK", &ct, read(&here.join(&f))),
                _ => set_error(400, "Not enough parameters or file not found"),
            }
        }
        "html/semantics/scripting-1/the-script-element/log.py" => {
            let (Some(id), Some(sec)) = (req.get_str("id").and_then(|v| v.parse::<i64>().ok()), req.get_str("sec").and_then(|v| v.parse::<u64>().ok())) else {
                return Some(Some(set_error(400, "Invalid parameter")));
            };
            sleep_ms(sec * 1000);
            Resp::new(200, "OK", "text/javascript", format!("log('{id}')").into_bytes())
        }
        "html/semantics/scripting-1/the-script-element/serve-custom-response.py" => {
            let key = req.get_str("key").unwrap_or_default();
            let (mut network_error, run_count, mut status_code, mut content_type) = match stash_take(&key, &dir) {
                Some(Stashed::Custom { network_error, run_count, status_code, content_type }) => (network_error, run_count, status_code, content_type),
                _ => (false, 0, 200, "application/javascript".to_string()),
            };
            let put = |ne: bool, rc: u32, sc: u16, ct: &str| {
                stash_put(&key, &dir, Stashed::Custom { network_error: ne, run_count: rc, status_code: sc, content_type: ct.to_string() });
            };
            match req.get_str("action").as_deref() {
                Some("stat") => {
                    put(network_error, run_count, status_code, &content_type);
                    return Some(Some(Resp::new(200, "OK", "text/plain", run_count.to_string().into_bytes())));
                }
                Some("reset-stat") => {
                    put(network_error, 0, status_code, &content_type);
                    return Some(Some(Resp::new(200, "OK", "text/plain", b"OK".to_vec())));
                }
                Some("set") => {
                    if let Some(c) = req.get_str("code").and_then(|v| v.parse().ok()) {
                        status_code = c;
                    }
                    if let Some(ct) = req.get_str("content-type") {
                        content_type = ct;
                    }
                    if let Some(ne) = req.get_str("network-error") {
                        network_error = ne.eq_ignore_ascii_case("true");
                    }
                    put(network_error, run_count, status_code, &content_type);
                    return Some(Some(Resp::new(200, "OK", "text/plain", b"OK".to_vec())));
                }
                _ => {}
            }
            if network_error {
                put(true, run_count + 1, status_code, &content_type);
                // The connection closes without a response.
                let _ = s.shutdown(std::net::Shutdown::Both);
                return Some(None);
            }
            let body: &[u8] = match content_type.as_str() {
                "application/javascript" => b"export default 'hello';",
                "application/json" => b"{\"hello\": \"world\"}",
                "text/css" => b"#test { background-color: rgb(0, 255, 0); }",
                _ => b"42",
            };
            put(network_error, run_count + 1, status_code, &content_type);
            Resp::new(status_code, "OK", &content_type, body.to_vec())
        }
        "html/semantics/scripting-1/the-script-element/resources/load-error-events.py"
        | "html/semantics/scripting-1/the-script-element/css-module/resources/load-error-events.py"
        | "html/semantics/scripting-1/the-script-element/json-module/load-error-events.py"
        | "html/semantics/scripting-1/the-script-element/text-module/load-error-events.py" => {
            let test = req.get_str("test").unwrap_or_default();
            if test.is_empty() || !test.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_') {
                return Some(Some(set_error(500, "AssertionError")));
            }
            let load = test.contains("_load");
            let (import_ok, import_404) = if rel.contains("/css-module/") {
                ("import \"./basic.css\" with { type: \"css\"}; ", "import \"./not_found.css\" with { type: \"css\"}; ")
            } else if rel.contains("/json-module/") {
                ("import \"./module.json\" with { type: \"json\"}; ", "import \"./not_found.json\" with { type: \"json\"}; ")
            } else if rel.contains("/text-module/") {
                ("import \"./file.txt\" with { type: \"text\" }; ", "import \"./not_found.txt\" with { type: \"text\" }; ")
            } else {
                ("\"use strict\"; ", "\"use strict\"; ")
            };
            let classic = !rel.contains("-module/");
            let (status, body) = if load {
                (200, format!("{import_ok}{test}.executed = true;"))
            } else {
                (if classic { 404 } else { 200 }, format!("{import_404}{test}.test.step(function() {{ assert_unreached(\"404 script should not be executed\"); }});"))
            };
            Resp::new(status, if status == 200 { "OK" } else { "Not Found" }, "text/javascript", body.into_bytes())
        }
        "html/semantics/scripting-1/the-script-element/module/resources/delayed-modulescript.py" => {
            let ms: f64 = req.get_str("ms").and_then(|v| v.parse().ok()).unwrap_or(500.0);
            sleep_ms(ms as u64);
            Resp::new(200, "OK", "text/javascript", b"export let delayedLoaded = true;".to_vec())
        }
        "html/semantics/scripting-1/the-script-element/module/resources/referrer-checker.py" => {
            Resp::new(200, "OK", "text/javascript", format!("export const referrer = '{referer}';").into_bytes()).header("Access-Control-Allow-Origin", "*")
        }
        "html/semantics/scripting-1/the-script-element/resources/cross-origin.py" => {
            let mut r = if req.method == "OPTIONS" {
                Resp::new(200, "OK", "", Vec::new())
            } else {
                let included = req.cookie("milk").as_deref() == Some("1");
                Resp::new(200, "OK", "text/javascript", format!("var included = {included};").into_bytes())
            };
            if let Some(o) = &origin {
                r = r.header("Access-Control-Allow-Origin", o).header("Access-Control-Allow-Methods", "GET").header("Access-Control-Allow-Credentials", "true");
            }
            r
        }
        "html/semantics/scripting-1/the-script-element/script-not-found-not-executed.py" => Resp::new(404, "Not Found", "text/javascript", b"test1_token = \"script executed\";".to_vec()),
        "html/semantics/scripting-1/the-script-element/text-module/serve-text-then-js.py" => {
            let Some(key) = req.get_str("key") else { return Some(Some(set_error(400, "Not enough parameters"))) };
            let n = stash_count(&key, &dir);
            stash_put(&key, &dir, Stashed::Str((n + 1).to_string()));
            if n == 0 {
                Resp::new(200, "OK", "text/plain", b"hello".to_vec())
            } else {
                Resp::new(200, "OK", "application/javascript", b"export default 'world';".to_vec())
            }
        }
        "html/semantics/scripting-1/the-script-element/json-module/http-accept-header-checker.py" => {
            let accept = req.header("accept").unwrap_or("");
            Resp::new(200, "OK", "application/json", format!("{{\"accept\": \"{accept}\"}}").into_bytes()).header("Access-Control-Allow-Origin", "*")
        }
        "html/semantics/scripting-1/the-script-element/text-module/http-accept-header-checker.py" => {
            Resp::new(200, "OK", "text/plain", req.header("accept").unwrap_or("").as_bytes().to_vec()).header("Access-Control-Allow-Origin", "*")
        }
        "html/semantics/scripting-1/the-script-element/css-module/resources/record-fetch.py" => {
            let (Some(key), Some(action)) = (req.get_str("key"), req.get_str("action")) else { return Some(Some(set_error(400, "Not enough parameters"))) };
            let n = stash_count(&key, &dir);
            match action.as_str() {
                "incCount" => {
                    stash_put(&key, &dir, Stashed::Str((n + 1).to_string()));
                    Resp::new(200, "OK", "text/css", b"#test { background-color: #FF0000; }".to_vec())
                }
                "getCount" => Resp::new(200, "OK", "text/json", format!("{{\"count\": {n} }}").into_bytes()),
                _ => set_error(400, "Invalid action"),
            }
        }
        "html/semantics/embedded-content/the-iframe-element/support/download_stash.py" => {
            let token = req.get_str("token").unwrap_or_default();
            if req.get("verify-token").is_some() {
                let hit = stash_take(&token, &dir).is_some();
                return Some(Some(Resp::new(200, "OK", "text/html", if hit { b"TOKEN_SET".to_vec() } else { b"TOKEN_NOT_SET".to_vec() }).header("X-Content-Type-Options", "nosniff")));
            }
            let Some(delay) = req.get_str("finish-delay").and_then(|v| v.parse::<f64>().ok()) else {
                // `<a download>`
                stash_put(&token, &dir, Stashed::Str("True".into()));
                return Some(Some(Resp::new(200, "OK", "text/html", Vec::new()).header("X-Content-Type-Options", "nosniff")));
            };
            // Navigation to a download: the body trickles in while the page cancels it.
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nX-Content-Type-Options: nosniff\r\nContent-Disposition: attachment\r\nConnection: close\r\n\r\n");
            let single = (delay / 10.0) as u64;
            for _ in 0..10 {
                sleep_ms(single);
                if s.write_all(b"\n").is_err() || s.flush().is_err() {
                    return Some(None);
                }
            }
            stash_put(&token, &dir, Stashed::Str("True".into()));
            return Some(None);
        }
        "html/semantics/embedded-content/the-img-element/404-response-with-actual-image-data.py" => Resp::new(
            404,
            "Not Found",
            "image/png",
            b64decode("iVBORw0KGgoAAAANSUhEUgAAABAAAAAQCAYAAAAf8/9hAAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsQAAA7EAZUrDhsAAAAhSURBVDhPY3wro/KfgQLABKXJBqMGjBoAAqMGDLwBDAwAEsoCTFWunmQAAAAASUVORK5CYII="),
        ),
        "html/semantics/embedded-content/the-img-element/resources/image-alternate-no-store.py" => {
            const STASH_ID: &str = "882e0112-2a43-4ff1-9503-658b1a2a8e67";
            let switch = stash_take_str(STASH_ID, &dir).as_deref() == Some("True");
            stash_put(STASH_ID, &dir, Stashed::Str(if switch { "False".into() } else { "True".into() }));
            let image = if switch { "blue-10.png" } else { "green.png" };
            Resp::new(200, "OK", "image/png", read(&here.join(image))).header("Cache-Control", "no-store")
        }
        "html/semantics/embedded-content/the-img-element/resources/referrer-checker-img.py" => {
            let expected = req.get_str("expected_referrer").unwrap_or_default();
            if referer == expected {
                Resp::new(200, "OK", "image/png", read(&here.join("image.png")))
            } else {
                Resp::new(404, "Not Found", "image/png", b"Not found".to_vec())
            }
        }
        "html/semantics/embedded-content/the-img-element/resources/image-and-stash.py" => {
            if let Some(uuid) = req.get_str("increment") {
                let n = stash_count(&uuid, &dir);
                stash_put(&uuid, &dir, Stashed::Str((n + 1).to_string()));
                Resp::new(200, "OK", "image/png", read(&here.join("image.png")))
            } else if let Some(uuid) = req.get_str("read") {
                let n = stash_count(&uuid, &dir);
                stash_put(&uuid, &dir, Stashed::Str(n.to_string()));
                Resp::new(200, "OK", "", n.to_string().into_bytes())
            } else {
                Resp::new(404, "Not Found", "", b"Not found".to_vec())
            }
        }
        "html/browsers/browsing-the-web/navigating-across-documents/resources/redirect.py" => {
            Resp::new(302, "Found", "", Vec::new()).header("Location", &req.get_str("location").unwrap_or_default())
        }
        "html/browsers/browsing-the-web/navigating-across-documents/refresh/resources/refresh.py" => {
            // The Refresh header carries the bytes 0x80 0xFF (a byte-to-Unicode test): written raw.
            let mut head = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nRefresh: 0;./refreshed.txt?".to_vec();
            head.extend_from_slice(&[0x80, 0xFF]);
            head.extend_from_slice(b"\r\nContent-Length: 15\r\nConnection: close\r\n\r\nNot refreshed.\n");
            let _ = s.write_all(&head);
            return Some(None);
        }
        "html/browsers/browsing-the-web/overlapping-navigations-and-traversals/resources/slow.py" => {
            sleep_ms(2000);
            Resp::new(200, "OK", "text/html", Vec::new())
        }
        "html/browsers/browsing-the-web/navigating-across-documents/resources/no-cache-single-redirect.py" => {
            let uuid = req.get_str("uuid").unwrap_or_default();
            if stash_take(&uuid, &dir).is_none() {
                stash_put(&uuid, &dir, Stashed::Str("sentinel value".into()));
                Resp::new(302, "Found", "", Vec::new()).header("Cache-Control", "no-store").header("Location", &req.get_str("location").unwrap_or_default())
            } else {
                Resp::new(200, "OK", "text/html", b"No redirect".to_vec()).header("Cache-Control", "no-store")
            }
        }
        "html/browsers/browsing-the-web/history-traversal/resources/unset_context_name_stash.py" => {
            let key = req.get_str("id").unwrap_or_default();
            if req.method == "POST" {
                let value = req.get_str("value").unwrap_or_default();
                stash_take(&key, &dir);
                stash_put(&key, &dir, Stashed::Str(value));
                Resp::new(200, "OK", "", b"OK".to_vec())
            } else {
                Resp::new(200, "OK", "", stash_take_str(&key, &dir).unwrap_or_else(|| "NONE".into()).into_bytes())
            }
        }
        "html/infrastructure/urls/resolving-urls/query-encoding/resources/blank.py" => {
            Resp::new(200, "OK", &format!("text/html; charset={}", req.get_str("encoding").unwrap_or_default()), Vec::new())
        }
        "html/infrastructure/urls/terminology-0/resources/echo-referrer-text.py" => Resp::new(200, "OK", "text/plain", referer.into_bytes()),
        "xhr/resources/echo-headers.py" => {
            // `str(request.raw_headers)`: the request's header lines as they came.
            let text: String = req.headers.iter().map(|(k, v)| format!("{k}: {v}\n")).collect();
            Resp::new(200, "OK", "text/plain", text.into_bytes())
        }
        "xhr/resources/infinite-redirects.py" => {
            let host = req.header("host").unwrap_or(HOST);
            let location = format!("http://{host}{}", req.path);
            let mut page = "alternate";
            let mut ty = 302;
            let mut mix = 0;
            if req.get_str("page").as_deref() == Some("alternate") {
                page = "default";
            }
            if req.get_str("type").as_deref() == Some("301") {
                ty = 301;
            }
            if req.get_str("mix").as_deref() == Some("1") {
                mix = 1;
                ty = if ty == 301 { 302 } else { 301 };
            }
            let new_location = format!("{location}?page={page}&type={ty}&mix={mix}");
            Resp::new(301, "Moved Permanently", "", format!("Hello guest. You have been redirected to {new_location}").into_bytes())
                .header("Cache-Control", "no-cache")
                .header("Pragma", "no-cache")
                .header("Location", &new_location)
        }
        "xhr/resources/parse-headers.py" => {
            let mut r = Resp::new(200, "OK", "", Vec::new());
            if let Some(v) = req.get_str("my-custom-header") {
                r = r.header("My-Custom-Header", &v);
            }
            r
        }
        "xhr/resources/json-with-bom.py" => Resp::new(200, "OK", "application/json", b"\xef\xbb\xbf{\"key\":\"value\"}".to_vec()),
        "xhr/resources/win-1252-xml.py" => Resp::new(200, "OK", "application/xml;charset=windows-1252", b"<\xff/>".to_vec()),
        "xhr/resources/invalid-utf8.py" => Resp::new(200, "OK", "application/json", b"{\"key\":\"\xff\"}".to_vec()),
        "xhr/resources/echo-content-type.py" => Resp::new(200, "OK", "text/plain", req.header("content-type").unwrap_or("").as_bytes().to_vec()).header("Connection", "close"),
        "xhr/resources/get-set-cookie.py" => {
            let expires = req.get("clear").filter(|v| !v.is_empty()).map(|_| http_date(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0) - 86400));
            let mut content = String::new();
            if let Some(c) = req.header("cookie") {
                for p in c.split(';') {
                    let p = p.trim();
                    if !p.is_empty() {
                        content.push(' ');
                        content.push_str(p);
                    }
                }
            }
            Resp::new(200, "OK", "text/plain", content.into_bytes())
                .header("Set-Cookie", &set_cookie_header("WK-test", "1", expires.as_deref(), false))
                .header("Set-Cookie", &set_cookie_header("WK-test-secure", "1", expires.as_deref(), true))
        }
        "xhr/resources/bad-chunk-encoding.py" | "fetch/api/resources/bad-chunk-encoding.py" => {
            let xhr = rel.starts_with("xhr/");
            let delay: u64 = if xhr { 100 } else { req.get_str("ms").and_then(|v| v.parse::<f64>().ok()).unwrap_or(1000.0) as u64 };
            let count: usize = if xhr { 5 } else { req.get_str("count").and_then(|v| v.parse().ok()).unwrap_or(50) };
            sleep_ms(delay);
            let head: &[u8] = if xhr {
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Type: text/plain\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n"
            } else {
                b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n"
            };
            let _ = s.write_all(head);
            sleep_ms(delay);
            for _ in 0..count {
                if s.write_all(b"a\r\nTEST_CHUNK\r\n").is_err() {
                    return Some(None);
                }
                sleep_ms(delay);
            }
            let _ = s.write_all(b"garbage");
            return Some(None);
        }
        "xhr/resources/upload.py" => {
            let post = req.post();
            let mut fields: Vec<(Vec<u8>, Vec<u8>)> = post.iter().filter(|(_, _, f, _)| f.is_none()).map(|(k, v, _, _)| (k.clone(), v.clone())).collect();
            fields.sort();
            let mut files: Vec<(Vec<u8>, String, String, usize)> = post.iter().filter_map(|(k, v, f, t)| f.as_ref().map(|f| (k.clone(), f.clone(), t.clone().unwrap_or_else(|| "None".into()), v.len()))).collect();
            files.sort();
            let mut content = Vec::new();
            let mut seen: Vec<Vec<u8>> = Vec::new();
            for (k, v) in fields {
                if seen.contains(&k) {
                    continue; // `values[0]`: the first value per name
                }
                seen.push(k.clone());
                content.extend_from_slice(&k);
                content.push(b'=');
                content.extend_from_slice(&v);
                content.push(b',');
            }
            content.push(b'\n');
            let mut seen: Vec<Vec<u8>> = Vec::new();
            for (k, f, t, len) in files {
                if seen.contains(&k) {
                    continue;
                }
                seen.push(k.clone());
                content.extend_from_slice(&k);
                content.extend_from_slice(format!("={f}:{t}:{len},").as_bytes());
            }
            Resp::new(200, "OK", "", content)
        }
        "xhr/resources/chunked.py" => {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nTrailer: X-Test-Me\r\nContent-Type: text/plain\r\nConnection: close\r\n\r\n");
            for chunk in [&b"First chunk\r\n"[..], b"Second chunk\r\n", b"Yet another (third) chunk\r\n", b"Yet another (fourth) chunk\r\n"] {
                let _ = s.write_all(format!("{:x}\r\n", chunk.len()).as_bytes());
                let _ = s.write_all(chunk);
                let _ = s.write_all(b"\r\n");
            }
            let _ = s.write_all(b"0\r\nX-Test-Me: Trailer header value\r\n\r\n");
            return Some(None);
        }
        "xhr/resources/last-modified.py" => {
            let src = here.join("well-formed.xml");
            let mtime = std::fs::metadata(&src).and_then(|m| m.modified()).ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_secs() as i64).unwrap_or(0);
            Resp::new(200, "OK", "application/xml", read(&src)).header("Last-Modified", &http_date(mtime))
        }
        "xhr/resources/accept.py" => Resp::new(200, "OK", "text/plain", req.header("accept").unwrap_or("NO").as_bytes().to_vec()),
        "xhr/resources/conditional.py" => {
            let tag = req.get_str("tag");
            let date = req.get_str("date").unwrap_or_default();
            let cors = req.get("cors").is_some();
            if req.method == "OPTIONS" {
                return Some(Some(Resp::new(200, "OK", "", Vec::new()).header("Access-Control-Allow-Origin", "*").header("Access-Control-Allow-Headers", "IF-NONE-MATCH")));
            }
            let mut r = Resp::new(200, "OK", "", Vec::new());
            if let Some(t) = &tag {
                r = r.header("ETag", &format!("\"{t}\""));
            } else if !date.is_empty() {
                r = r.header("Last-Modified", &date);
            }
            if cors {
                r = r.header("Access-Control-Allow-Origin", "*");
            }
            let matched = req.header("if-none-match").is_some_and(|m| tag.as_deref() == Some(m)) || req.header("if-modified-since").is_some_and(|m| m == date);
            if matched {
                r.status = 304;
                r.reason = "SUPERCOOL".into();
                r
            } else {
                if !cors {
                    r = r.header("Access-Control-Allow-Origin", "*");
                }
                r.headers.push(("Content-Type".into(), "text/plain".into()));
                r.body = b"MAYBE NOT".to_vec();
                r
            }
        }
        "xhr/resources/form.py" => {
            let post = req.post();
            let first = |name: &[u8]| post.iter().find(|(k, _, _, _)| k == name).map(|(_, v, _, _)| v.clone()).unwrap_or_default();
            let mut body = b"id:".to_vec();
            body.extend(first(b"id"));
            body.extend_from_slice(b";value:");
            body.extend(first(b"value"));
            body.push(b';');
            Resp::new(200, "OK", "", body)
        }
        "resource-timing/resources/fake_responses.py" => {
            if req.method == "OPTIONS" {
                return Some(Some(Resp::new(204, "No Content", "", Vec::new()).header("Access-Control-Allow-Headers", "*").header("Access-Control-Allow-Origin", "*")));
            }
            let tag = req.get_str("tag");
            let date = req.get_str("date").unwrap_or_default();
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Access-Control-Allow-Origin", "*").header("Timing-Allow-Origin", "*");
            if let Some(t) = &tag {
                r = r.header("ETag", &format!("\"{t}\""));
            } else if !date.is_empty() {
                r = r.header("Last-Modified", &date);
            }
            if let Some(loc) = req.get_str("redirect") {
                r.status = 302;
                r.reason = "Moved".into();
                return Some(Some(r.header("Location", &loc)));
            }
            if let Some(url) = req.get_str("url") {
                let name = url.split('?').next().unwrap_or("").to_string();
                if !name.contains("..") {
                    r.headers.push(("Content-Type".into(), "text/javascript".into()));
                    r.body = read(&root.join("resource-timing/resources").join(name));
                    return Some(Some(r));
                }
            }
            let matched = req.header("if-none-match").is_some_and(|m| tag.as_deref() == Some(m)) || req.header("if-modified-since").is_some_and(|m| m == date);
            if matched {
                r.status = 304;
                r.reason = "SUPERCOOL".into();
                r
            } else {
                r.headers.push(("Content-Type".into(), "text/plain".into()));
                r.body = b"MAYBE NOT".to_vec();
                r
            }
        }
        "resource-timing/resources/empty.py" => Resp::new(200, "OK", "text/plain", Vec::new()),
        "resource-timing/resources/TAOResponse.py" => {
            let o = origin.clone().unwrap_or_default();
            let tao = req.get_str("tao").unwrap_or_default();
            let mut r = Resp::new(200, "OK", "", Vec::new());
            if origin.is_some() {
                r = r.header("Access-Control-Allow-Origin", &o);
            }
            match tao.as_str() {
                "wildcard" => r = r.header("Timing-Allow-Origin", "*"),
                "null" => r = r.header("Timing-Allow-Origin", "null"),
                "Null" => r = r.header("Timing-Allow-Origin", "Null"),
                "origin" => r = r.header("Timing-Allow-Origin", &o),
                t if t.starts_with("origin_port") => {
                    let mut parts = o.splitn(3, ':');
                    let host = format!("{}:{}", parts.next().unwrap_or(""), parts.next().unwrap_or(""));
                    let port = t.strip_prefix("origin_port_").unwrap_or("");
                    r = r.header("Timing-Allow-Origin", &format!("{host}:{port}"));
                }
                "space" => r = r.header("Timing-Allow-Origin", &format!("{o} *")),
                "multi" => r = r.header("Timing-Allow-Origin", &o).header("Timing-Allow-Origin", "*"),
                "multi_wildcard" => r = r.header("Timing-Allow-Origin", "*").header("Timing-Allow-Origin", "*"),
                "match_origin" => r = r.header("Timing-Allow-Origin", &o).header("Timing-Allow-Origin", "fake"),
                "match_wildcard" => r = r.header("Timing-Allow-Origin", "fake").header("Timing-Allow-Origin", "*"),
                "uppercase" => r = r.header("Timing-Allow-Origin", &o.to_uppercase()),
                _ => {}
            }
            if req.get("img").is_some() {
                r.headers.push(("Content-Type".into(), "image/png".into()));
                r.body = read(&root.join("resource-timing/resources/blue.png"));
            } else {
                r.headers.push(("Content-Type".into(), "text/plain".into()));
                r.body = b"TEST".to_vec();
            }
            r
        }
        "resource-timing/resources/status-code.py" => {
            let status: u16 = req.get_str("status").and_then(|v| v.parse().ok()).unwrap_or(200);
            let mut r = Resp::new(status, "", "", Vec::new());
            if let Some(t) = req.get_str("tao_value") {
                r = r.header("timing-allow-origin", &t);
            }
            if let Some(a) = req.get_str("allow_origin") {
                r = r.header("access-control-allow-origin", &a);
            }
            r
        }
        "resource-timing/resources/redirect-without-location.py" => Resp::new(302, "Found", "", Vec::new()),
        "resource-timing/resources/resource-timing-content-length.py" => {
            let content = req.get("content").unwrap_or_default();
            let length = req.get_str("length").unwrap_or_default();
            let mut out = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain;charset=UTF-8\r\nConnection: close\r\n".to_vec();
            if length == "auto" {
                out.extend_from_slice(format!("Content-Length:{}\r\n", content.len()).as_bytes());
            } else if length != "none" {
                out.extend_from_slice(format!("Content-Length:{length}\r\n").as_bytes());
            }
            out.extend_from_slice(b"\r\n");
            out.extend_from_slice(&content);
            let _ = s.write_all(&out);
            return Some(None);
        }
        "resource-timing/resources/redirect-cors.py" | "resource-timing/resources/cacheable-and-validated.py" => {
            let cacheable = rel.ends_with("cacheable-and-validated.py");
            let mut r = Resp::new(200, "OK", "", Vec::new());
            if cacheable {
                if let Some(t) = req.get_str("timing_allow_origin").filter(|t| !t.is_empty()) {
                    r = r.header("Timing-Allow-Origin", &t);
                }
                if let Some(o) = &origin {
                    r = r.header("Access-Control-Allow-Origin", o);
                }
                r = r.header("Cache-Control", "max-age=60").header("ETag", "assdfsdfe");
            } else {
                if let Some(a) = req.get_str("allow_origin") {
                    r = r.header("Access-Control-Allow-Origin", &a);
                }
                if let Some(t) = req.get_str("timing_allow_origin") {
                    r = r.header("Timing-Allow-Origin", &t);
                }
            }
            if req.method == "OPTIONS" {
                r.status = 204;
                r.reason = "No Content".into();
                if let Some(m) = req.header("access-control-request-method") {
                    r = r.header("Access-Control-Allow-Methods", m);
                }
                if let Some(h) = req.header("access-control-request-headers") {
                    r = r.header("Access-Control-Allow-Headers", h);
                }
                r
            } else if cacheable {
                if req.header("cache-control").is_some() {
                    r.status = 304;
                    r.reason = "NotModified".into();
                } else {
                    r.body = req.get("content").unwrap_or_default();
                }
                r
            } else {
                r.status = 302;
                r.reason = "Found".into();
                r.header("Location", &req.get_str("location").unwrap_or_default())
            }
        }
        "encoding/resources/single-byte-raw.py" => Resp::new(200, "OK", &format!("text/plain;charset={}", req.get_str("label").unwrap_or_default()), (0u8..255).collect::<Vec<u8>>()),
        "encoding/resources/json-charset.py" => {
            let mut ct = req.get_str("type").unwrap_or_else(|| "application/json".into());
            if let Some(cs) = req.get_str("charset") {
                ct.push_str(&format!(";charset={cs}"));
            }
            Resp::new(200, "OK", &ct, "{\"text\":\"☃é\"}".as_bytes().to_vec()).header("X-Content-Type-Options", "nosniff")
        }
        "encoding/resources/text-plain-charset.py" => Resp::new(200, "OK", &format!("text/plain;charset={}", req.get_str("label").unwrap_or_default()), b"hello encoding".to_vec()),
        "encoding/resources/text-html-meta-charset.py" => Resp::new(200, "OK", "text/html", format!("<meta charset=\"{}\">hello encoding", req.get_str("label").unwrap_or_default()).into_bytes()),
        "url/resources/percent-encoding.py" => {
            let value = req.get_str("value").unwrap_or_default().replace(' ', "+");
            let encoding = req.get_str("encoding").unwrap_or_default();
            let decoded = String::from_utf8_lossy(&b64decode(&value)).into_owned();
            let refs: String = decoded.chars().map(|c| format!("&#x{:X};", c as u32)).collect();
            Resp::new(200, "OK", &format!("text/html;charset={encoding}"), format!("<!doctype html>\n<a href=\"https://doesnotmatter.invalid/?{refs}#{refs}\">test</a>\n").into_bytes())
        }
        "fetch/api/resources/script-with-header.py" => {
            let content: &[u8] = if req.get_str("content").as_deref() == Some("empty") { b"" } else { b"console.log('Script loaded')" };
            Resp::new(200, "OK", &req.get_str("mime").unwrap_or_default(), content.to_vec())
        }
        "fetch/api/resources/cache.py" => {
            if req.header("if-none-match") == Some("\"123abc\"") {
                Resp::new(304, "Not Modified", "", Vec::new()).header("X-HTTP-STATUS", "304")
            } else {
                Resp::new(200, "OK", "text/plain", b"lorem ipsum dolor sit amet".to_vec()).header("ETag", "\"123abc\"")
            }
        }
        "fetch/api/resources/redirect-empty-location.py" => Resp::new(302, "Found", "", Vec::new()).header("Location", ""),
        "fetch/api/resources/stashed-chunk.py" => {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n");
            for (c, wait) in [(b'A', 100), (b'B', 100), (b'C', 5000)] {
                if s.write_all(&[c; 4096]).is_err() || s.flush().is_err() {
                    return Some(None);
                }
                sleep_ms(wait);
            }
            return Some(None);
        }
        "eventsource/resources/cors.py" => {
            let o = req.get_str("origin").or_else(|| origin.clone()).unwrap_or_default();
            let credentials = req.get_str("credentials").unwrap_or_else(|| "true".into());
            let cors = |r: Resp| r.header("Access-Control-Allow-Origin", &o).header("Access-Control-Allow-Credentials", &credentials);
            match req.get_str("run").as_deref() {
                Some("cache-control") => {
                    let text = String::from_utf8_lossy(&read(&root.join("eventsource/resources/cache-control.event_stream"))).into_owned();
                    cors(Resp::new(200, "OK", es, subst(&text, req, ports).into_bytes()))
                }
                Some("redirect") => match py("common/redirect.py", req, s, root, ports) {
                    Some(Some(r)) => cors(r),
                    other => return other,
                },
                Some(h @ ("status-reconnect" | "message")) => match py(&format!("eventsource/resources/{h}.py"), req, s, root, ports) {
                    Some(Some(r)) => cors(r),
                    other => return other,
                },
                _ => cors(Resp::new(200, "OK", "", Vec::new())),
            }
        }
        "eventsource/resources/cors-cookie.py" => {
            let last = req.header("last-event-id").unwrap_or("").to_string();
            let ident = req.get_str("ident").unwrap_or_else(|| "test".into());
            let cookie = if req.cookie(&ident).is_some() { "COOKIE" } else { "NO_COOKIE" };
            let o = req.get_str("origin").or_else(|| origin.clone()).unwrap_or_default();
            let credentials = req.get_str("credentials").unwrap_or_else(|| "true".into());
            let mut r = Resp::new(200, "OK", "", Vec::new());
            if o != "none" {
                r = r.header("Access-Control-Allow-Origin", &o);
            }
            if credentials != "none" {
                r = r.header("Access-Control-Allow-Credentials", &credentials);
            }
            match last.as_str() {
                "" => {
                    r.headers.push(("Content-Type".into(), es.into()));
                    r = r.header("Set-Cookie", &set_cookie_header(&ident, "COOKIE", None, false));
                    r.body = format!("id: 1\nretry: 200\ndata: first {cookie}\n\n").into_bytes();
                }
                "1" => {
                    r.headers.push(("Content-Type".into(), es.into()));
                    r = r.header("Set-Cookie", &set_cookie_header(&ident, "COOKIE", Some(&http_date(996_192_000)), false));
                    r.body = format!("id: 2\ndata: second {cookie}\n\n").into_bytes();
                }
                _ => {
                    r.headers.push(("Content-Type".into(), "stop".into()));
                    r.body = format!("data: {last}{cookie}\n\n").into_bytes();
                }
            }
            r
        }
        "FileAPI/file/resources/echo-content-escaped.py" => {
            let mut content = Vec::new();
            for &b in &req.body {
                if b <= 0x1F || b >= 0x7F {
                    content.extend_from_slice(format!("\\x{b:02x}").as_bytes());
                } else if b == b'\\' {
                    content.extend_from_slice(b"\\\\");
                } else {
                    content.push(b);
                }
            }
            let text = String::from_utf8_lossy(&content).replace("\\x0d\\x0a", "\r\n");
            Resp::new(200, "OK", "text/plain; charset=UTF-8", text.into_bytes())
                .header("X-Request-Method", &req.method)
                .header("X-Request-Content-Length", req.header("content-length").unwrap_or("NO"))
                .header("X-Request-Content-Type", req.header("content-type").unwrap_or("NO"))
        }
        "websockets/cookies/support/set-cookie.py" => {
            let unquoted: String = pct(&req.query.replace('+', "%2B")).iter().map(|&c| c as char).collect();
            Resp::new(200, "OK", "text/plain", Vec::new()).header("Set-Cookie", &unquoted)
        }
        "websockets/opening-handshake/resources/redirect_response.py" => Resp::new(301, "moved", "", Vec::new()).header("Location", "/echo"),
        "websockets/security/check.py" => Resp::new(200, "OK", "", if req.header("sec-websocket-key").is_some() { b"FAIL".to_vec() } else { b"PASS".to_vec() }),
        // ---- AETHERWPT5 (SR144): the CORS handlers the cross-origin lift reaches (each from its source).
        "fetch/api/resources/preflight.py" => {
            let mut r = Resp::new(200, "OK", "text/plain", Vec::new());
            let token = req.get_str("token");
            match req.get_str("origin") {
                Some(o) => {
                    for part in o.split(", ") {
                        r = r.header("Access-Control-Allow-Origin", part);
                    }
                }
                None => r = r.header("Access-Control-Allow-Origin", "*"),
            }
            let mut data: Vec<(String, String)> = vec![("control_request_headers".into(), String::new()), ("preflight".into(), "0".into()), ("preflight_referrer".into(), String::new())];
            let set = |d: &mut Vec<(String, String)>, k: &str, v: String| {
                d.retain(|(x, _)| x != k);
                d.push((k.to_string(), v));
            };
            let get = |d: &[(String, String)], k: &str| d.iter().find(|(x, _)| x == k).map(|(_, v)| v.clone());
            if req.get("clear-stash").is_some() {
                let hit = token.as_deref().is_some_and(|t| stash_take(t, &dir).is_some());
                r.body = if hit { b"1".to_vec() } else { b"0".to_vec() };
                return Some(Some(r));
            }
            if req.get("credentials").is_some() {
                r = r.header("Access-Control-Allow-Credentials", "true");
            }
            if req.method == "OPTIONS" {
                if req.header("access-control-request-method").is_none() {
                    return Some(Some(set_error(400, "No Access-Control-Request-Method header")));
                }
                if req.header("accept").unwrap_or("") != "*/*" {
                    return Some(Some(set_error(400, "Request does not have 'Accept: */*' header")));
                }
                if req.get("control_request_headers").is_some() {
                    set(&mut data, "control_request_headers", req.header("access-control-request-headers").unwrap_or("").to_string());
                }
                if let Some(m) = req.get_str("max_age") {
                    r = r.header("Access-Control-Max-Age", &m);
                }
                if let Some(h) = req.get_str("allow_headers") {
                    r = r.header("Access-Control-Allow-Headers", &h);
                }
                if let Some(m) = req.get_str("allow_methods") {
                    r = r.header("Access-Control-Allow-Methods", &m);
                }
                let status: u16 = req.get_str("preflight_status").and_then(|v| v.parse().ok()).unwrap_or(200);
                set(&mut data, "preflight", "1".into());
                set(&mut data, "preflight_referrer", referer.clone());
                set(&mut data, "preflight_user_agent", req.header("user-agent").unwrap_or("").to_string());
                if let Some(t) = &token {
                    stash_put(t, &dir, Stashed::Dict(data));
                }
                r.status = status;
                return Some(Some(r));
            }
            if let Some(t) = &token {
                if let Some(Stashed::Dict(d)) = stash_take(t, &dir) {
                    data = d;
                }
            }
            if req.get("checkUserAgentHeaderInPreflight").is_some() && req.header("user-agent").map(str::to_string) != get(&data, "preflight_user_agent") {
                r.status = 400;
                r.body = b"ERROR: No user-agent header in preflight".to_vec();
                return Some(Some(r));
            }
            r = r.header("Access-Control-Expose-Headers", "x-did-preflight, x-control-request-headers, x-referrer, x-preflight-referrer, x-origin");
            r = r.header("x-did-preflight", &get(&data, "preflight").unwrap_or_else(|| "0".into()));
            r = r.header("x-control-request-headers", &get(&data, "control_request_headers").unwrap_or_default());
            r = r.header("x-preflight-referrer", &get(&data, "preflight_referrer").unwrap_or_default());
            r = r.header("x-referrer", &referer);
            r = r.header("x-origin", origin.as_deref().unwrap_or(""));
            if let Some(t) = &token {
                stash_put(t, &dir, Stashed::Dict(data));
            }
            r
        }
        "xhr/resources/reset-token.py" => {
            let token = req.get_str("token").unwrap_or_default();
            stash_put(&token, &dir, Stashed::Str(String::new()));
            Resp::new(200, "OK", "", b"PASS".to_vec()).header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or(""))
        }
        "xhr/resources/access-control-preflight-denied.py" => {
            let token = req.get_str("token").unwrap_or_default();
            let state = stash_take_str(&token, &dir).filter(|s| !s.is_empty()).unwrap_or_else(|| "Uninitialized".into());
            let command = req.get_str("command");
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Cache-Control", "no-store").header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or("")).header("Access-Control-Max-Age", "1");
            let fail = |r: &mut Resp, m: &str| {
                r.body = format!("FAIL: {m}").into_bytes();
                r.status = 400;
            };
            let set_state = |v: &str| {
                stash_put(&token, &dir, Stashed::Str(v.into()));
            };
            if command.as_deref() == Some("reset") {
                if req.method == "GET" {
                    set_state("");
                    r.body = b"Server state reset".to_vec();
                } else {
                    fail(&mut r, "Invalid Method.");
                }
            } else if state == "Uninitialized" {
                if req.method == "OPTIONS" {
                    r.body = b"This request should not be displayed.".to_vec();
                    set_state("Denied");
                } else {
                    fail(&mut r, &state);
                }
            } else if state == "Denied" {
                if req.method == "GET" && command.as_deref() == Some("complete") {
                    set_state("");
                    r.body = b"Request successfully blocked.".to_vec();
                } else {
                    set_state("Deny Ignored");
                    fail(&mut r, "The request was not denied.");
                }
            } else if state == "Deny Ignored" {
                set_state("");
                fail(&mut r, &state);
            } else {
                set_state("");
                fail(&mut r, "Unknown Error.");
            }
            r
        }
        "xhr/resources/access-control-basic-put-allow.py" => {
            let o = origin.as_deref().unwrap_or("");
            match req.method.as_str() {
                "OPTIONS" => Resp::new(200, "OK", "text/plain", Vec::new()).header("Access-Control-Allow-Credentials", "true").header("Access-Control-Allow-Methods", "PUT").header("Access-Control-Allow-Origin", o),
                "PUT" => {
                    let mut body = b"PASS: Cross-domain access allowed.\n".to_vec();
                    body.extend_from_slice(&req.body);
                    Resp::new(200, "OK", "text/plain", body).header("Access-Control-Allow-Credentials", "true").header("Access-Control-Allow-Origin", o)
                }
                m => Resp::new(200, "OK", "text/plain", format!("Wrong method: {m}").into_bytes()),
            }
        }
        // `response.text = …` sets nothing wptserve writes: an empty 200 without CORS headers.
        "xhr/resources/access-control-basic-denied.py" => Resp::new(200, "OK", "text/plain", Vec::new()).header("Cache-Control", "no-store"),
        "xhr/resources/access-control-basic-options-not-supported.py" => {
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Cache-Control", "no-store");
            if req.method != "OPTIONS" {
                match &origin {
                    Some(o) => r = r.header("Access-Control-Allow-Credentials", "true").header("Access-Control-Allow-Origin", o),
                    None => r.status = 500,
                }
            } else {
                r.status = 400;
            }
            r
        }
        "xhr/resources/access-control-preflight-request-invalid-status.py" => {
            let mut r = Resp::new(200, "OK", "", Vec::new());
            if req.method == "OPTIONS" {
                if let Some(c) = req.get_str("code").and_then(|v| v.parse::<u16>().ok()).filter(|c| *c != 0) {
                    r.status = c;
                }
                r = r.header("Access-Control-Max-Age", "1").header("Access-Control-Allow-Headers", "x-pass");
            }
            r.header("Cache-Control", "no-store").header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or(""))
        }
        "xhr/resources/access-control-basic-allow-star.py" => Resp::new(200, "OK", "text/plain", b"PASS: Cross-domain access allowed.".to_vec()).header("Access-Control-Allow-Origin", "*"),
        "xhr/resources/access-control-basic-allow.py" => Resp::new(200, "OK", "text/plain", b"PASS: Cross-domain access allowed.".to_vec())
            .header("Access-Control-Allow-Credentials", "true")
            .header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or("")),
        "xhr/resources/access-control-origin-header.py" => Resp::new(200, "OK", "text/plain", format!("PASS: Cross-domain access allowed.\nHTTP_ORIGIN: {}", origin.as_deref().unwrap_or("")).into_bytes())
            .header("Cache-Control", "no-cache, no-store")
            .header("Access-Control-Allow-External", "true")
            .header("Access-Control-Allow-Origin", "*"),
        "xhr/resources/access-control-allow-with-body.py" => Resp::new(200, "OK", "text/plain", b"PASS".to_vec())
            .header("Cache-Control", "no-store")
            .header("Access-Control-Allow-Headers", "X-Requested-With")
            .header("Access-Control-Max-Age", "0")
            .header("Access-Control-Allow-Origin", "*")
            .header("Access-Control-Allow-Methods", "*")
            .header("Vary", "Accept-Encoding"),
        "xhr/resources/access-control-basic-cors-safelisted-request-headers.py" => {
            if req.method != "POST" {
                return Some(Some(Resp::new(400, "Bad Request", "", Vec::new()).header("Cache-Control", "no-store")));
            }
            let mut body = String::new();
            for h in ["Accept", "Accept-Language", "Content-Language", "Content-Type"] {
                body.push_str(&format!("{h}: {}\n", req.header(h).unwrap_or("<None>")));
            }
            Resp::new(200, "OK", "", body.into_bytes()).header("Cache-Control", "no-store").header("Access-Control-Allow-Credentials", "true").header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or(""))
        }
        "xhr/resources/access-control-basic-cors-safelisted-response-headers.py" => Resp::new(200, "OK", "text/plain", b"PASS: Cross-domain access allowed.".to_vec())
            .header("cache-control", "no cache")
            .header("content-language", "en")
            .header("expires", "Fri, 30 Oct 1998 14:19:41 GMT")
            .header("last-modified", "Tue, 15 Nov 1994 12:45:26 GMT")
            .header("pragma", "no-cache")
            .header("x-test", "foobar")
            .header("Access-Control-Allow-Origin", "*"),
        "xhr/resources/access-control-preflight-request-allow-headers-returns-star.py" | "xhr/resources/access-control-preflight-request-header-returns-origin.py" => {
            let star = rel.ends_with("returns-star.py");
            if req.method == "OPTIONS" {
                let allow_origin = if star { "*".to_string() } else { origin.clone().unwrap_or_default() };
                let allow_headers = if star { "*" } else { "X-Test" };
                Resp::new(200, "OK", "", Vec::new()).header("Access-Control-Allow-Origin", &allow_origin).header("Access-Control-Allow-Headers", allow_headers)
            } else if req.header("x-test").is_some() {
                Resp::new(200, "OK", "text/plain", b"PASS".to_vec()).header("Access-Control-Allow-Origin", "*")
            } else {
                Resp::new(400, "Bad Request", "", Vec::new()).header("Access-Control-Allow-Origin", "*")
            }
        }
        "xhr/resources/access-control-preflight-request-header-lowercase.py" => {
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Cache-Control", "no-store").header("Access-Control-Allow-Origin", "*").header("Access-Control-Max-Age", "0");
            if req.method == "OPTIONS" {
                let asks = req.header("access-control-request-headers").unwrap_or("").split(',').any(|h| h.trim() == "x-test");
                if asks {
                    r = r.header("Access-Control-Allow-Headers", "X-Test");
                } else {
                    r.status = 400;
                }
            } else if req.header("x-test").is_some() {
                r.body = b"PASS".to_vec();
            } else {
                r.status = 400;
            }
            r
        }
        "xhr/resources/access-control-preflight-request-header-sorted.py" => {
            let headers = "x-custom-s,x-custom-test,x-custom-u,x-custom-ua,x-custom-v";
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Cache-Control", "no-store").header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or(""));
            if req.method == "OPTIONS" {
                r = r.header("Access-Control-Max-Age", "0").header("Access-Control-Allow-Headers", headers);
                if req.header("access-control-request-headers") != Some(headers) {
                    r.status = 400;
                }
            } else if req.header("x-custom-s").is_some() {
                r.body = b"PASS".to_vec();
            } else {
                r.status = 400;
                r.body = b"FAIL".to_vec();
            }
            r
        }
        "xhr/resources/access-control-preflight-request-headers-origin.py" => {
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Cache-Control", "no-store").header("Access-Control-Allow-Origin", "*");
            if req.method == "OPTIONS" {
                if req.header("access-control-request-headers").unwrap_or("").to_ascii_lowercase().contains("origin") {
                    r.status = 400;
                    r.body = b"Error: 'origin' included in Access-Control-Request-Headers".to_vec();
                } else {
                    r = r.header("Access-Control-Allow-Headers", "x-pass");
                }
            } else {
                r.body = req.header("x-pass").unwrap_or("").as_bytes().to_vec();
            }
            r
        }
        "xhr/resources/access-control-cookie.py" => {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Cache-Control", "no-store").header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or("")).header("Access-Control-Allow-Credentials", "true");
            if let Some(c) = req.header("cookie") {
                let names: Vec<String> = c.split(';').filter_map(|p| p.trim().split_once('=').map(|(k, _)| k.to_string())).collect();
                for name in names {
                    r = r.header("Set-Cookie", &set_cookie_header(&name, "deleted", Some(&http_date(now - 86400)), false));
                }
            }
            if let Some(name) = req.get_str("cookie_name").filter(|n| !n.is_empty()) {
                r = r.header("Set-Cookie", &set_cookie_header(&name, "COOKIE", Some(&http_date(now + 86400)), false));
            }
            r
        }
        "xhr/resources/echo-content-cors.py" => {
            let mut r = Resp::new(200, "OK", "text/plain", req.body.clone())
                .header("X-Request-Method", &req.method)
                .header("X-Request-Content-Length", req.header("content-length").unwrap_or("NO"))
                .header("X-Request-Content-Type", req.header("content-type").unwrap_or("NO"))
                .header("Access-Control-Allow-Credentials", "true");
            // The handler reads `origin` from the query for all three (its source's quirk).
            let o = req.get_str("origin");
            if let Some(v) = o.clone().or_else(|| origin.clone()) {
                r = r.header("Access-Control-Allow-Origin", &v);
            }
            if let Some(v) = o.clone().or_else(|| req.header("access-control-request-headers").map(str::to_string)) {
                r = r.header("Access-Control-Allow-Headers", &v);
            }
            if let Some(v) = o.or_else(|| req.header("access-control-request-method").map(str::to_string)) {
                r = r.header("Access-Control-Allow-Methods", &format!("OPTIONS, {v}"));
            }
            r
        }
        "xhr/resources/redirect-cors.py" => {
            let location = req.get_str("location").unwrap_or_default();
            let mut r = Resp::new(200, "OK", "", Vec::new());
            if req.method == "OPTIONS" {
                if req.get("redirect_preflight").is_some() {
                    r.status = 302;
                    r.reason = "Found".into();
                    r = r.header("Location", &location);
                }
                r = r.header("Access-Control-Allow-Methods", "GET").header("Access-Control-Max-Age", "1");
            } else if req.method == "GET" {
                r.status = 302;
                r.reason = "Found".into();
                r = r.header("Location", &location);
            }
            if req.get("allow_origin").is_some() {
                r = r.header("Access-Control-Allow-Origin", origin.as_deref().unwrap_or(""));
            }
            if let Some(h) = req.get_str("allow_header") {
                r = r.header("Access-Control-Allow-Headers", &h);
            }
            r
        }
        "xhr/resources/header-user-agent.py" => {
            let mut r = Resp::new(200, "OK", "", Vec::new()).header("Access-Control-Allow-Origin", "*").header("Access-Control-Max-Age", "0").header("Access-Control-Allow-Headers", "x-test");
            let ua = req.header("user-agent").is_some_and(|u| !u.is_empty());
            if req.method == "OPTIONS" {
                if !ua {
                    r.status = 400;
                    r.body = b"FAIL: User-Agent header missing in preflight request.".to_vec();
                }
            } else if ua {
                r.body = b"PASS".to_vec();
            } else {
                r.status = 400;
                r.body = b"FAIL: User-Agent header missing in request".to_vec();
            }
            r
        }
        // `chr(0xff)` as a str: wptserve encodes it UTF-8 (two bytes).
        "xhr/resources/win-1252-html.py" => Resp::new(200, "OK", "text/html;charset=windows-1252", "\u{ff}".as_bytes().to_vec()),
        "xhr/resources/invalid-utf8-html.py" => Resp::new(200, "OK", "text/html;charset=utf-8", vec![0xff]),
        "resource-timing/resources/multi_redirect.py" => {
            let step: u32 = req.get_str("step").and_then(|v| v.parse().ok()).unwrap_or(1);
            let page_origin = req.get_str("page_origin").unwrap_or_default();
            let cross_origin = req.get_str("cross_origin").unwrap_or_default();
            let final_resource = req.get_str("final_resource").unwrap_or_default();
            let tao_value = req.get_str("tao_value").unwrap_or_else(|| "*".into());
            let tao_steps: i64 = req.get_str("tao_steps").and_then(|v| v.parse().ok()).unwrap_or(0);
            let quoted: String = final_resource.bytes().map(|b| if b.is_ascii_alphanumeric() || b"_.-~/".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
            let path = format!("/resource-timing/resources/multi_redirect.py?page_origin={page_origin}&cross_origin={cross_origin}&final_resource={quoted}&tao_value={tao_value}&tao_steps={}&step=", tao_steps - 1);
            let mut r = Resp::new(302, "Found", "", Vec::new());
            if tao_steps > 0 {
                r = r.header("timing-allow-origin", &tao_value);
            }
            let url = match step {
                1 => format!("{cross_origin}{path}2"),
                2 => format!("{page_origin}{path}3"),
                _ => format!("{page_origin}{final_resource}"),
            };
            r.header("Location", &url)
        }
        "common/slow-redirect.py" | "html/semantics/scripting-1/the-script-element/module/dynamic-import/beta/redirect.py" => {
            let status: u16 = req.get_str("status").and_then(|v| v.parse().ok()).unwrap_or(302);
            if rel.starts_with("common/") {
                let delay: u64 = req.get_str("delay").and_then(|v| v.parse().ok()).unwrap_or(2);
                sleep_ms(delay * 1000);
            }
            Resp::new(status, "Redirect", "", Vec::new()).header("Location", &req.get_str("location").unwrap_or_default())
        }
        _ => return None,
    }))
}

fn http_conn(mut s: TcpStream, root: PathBuf, ports: Ports, report_js: &'static str) {
    let Some(req) = read_req(&mut s) else { return };
    let rel = req.path.trim_start_matches('/').to_string();
    let resp = if rel == "resources/testharnessreport.js" {
        Resp::new(200, "OK", "text/javascript", report_js.as_bytes().to_vec())
    } else if rel == "resources/testdriver-vendor.js" {
        // AETHERWPT5 (SR144): the checkout's vendor file is empty; Aether's drives the engine.
        Resp::new(200, "OK", "text/javascript", TESTDRIVER_VENDOR_JS.as_bytes().to_vec())
    } else if let Some(stem) = rel.strip_suffix(".any.html") {
        match wrapper(&root, stem, "any", report_js) {
            Some(b) => Resp::new(200, "OK", "text/html; charset=utf-8", b),
            None => Resp::new(404, "Not Found", "text/plain", b"not found".to_vec()),
        }
    } else if let Some(stem) = rel.strip_suffix(".window.html") {
        match wrapper(&root, stem, "window", report_js) {
            Some(b) => Resp::new(200, "OK", "text/html; charset=utf-8", b),
            None => Resp::new(404, "Not Found", "text/plain", b"not found".to_vec()),
        }
    } else if let Some(stem) = rel.strip_suffix(".any.worker.html") {
        // AETHERWORKERS3 (SR129): the worker-global variant of an `.any.js` test.
        match worker_wrapper(&root, stem, "any.worker.js", "any.js") {
            Some(b) => Resp::new(200, "OK", "text/html; charset=utf-8", b),
            None => Resp::new(404, "Not Found", "text/plain", b"not found".to_vec()),
        }
    } else if let Some(stem) = rel.strip_suffix(".any.worker.js") {
        match any_worker_script(&root, stem) {
            Some(b) => Resp::new(200, "OK", "text/javascript; charset=utf-8", b),
            None => Resp::new(404, "Not Found", "text/plain", b"not found".to_vec()),
        }
    } else if let Some(stem) = rel.strip_suffix(".worker.html") {
        // AETHERWORKERS3 (SR129): a `.worker.js` test's page (the `.worker.js` itself is served as a file).
        match worker_wrapper(&root, stem, "worker.js", "worker.js") {
            Some(b) => Resp::new(200, "OK", "text/html; charset=utf-8", b),
            None => Resp::new(404, "Not Found", "text/plain", b"not found".to_vec()),
        }
    } else if rel.ends_with(".py") {
        match py(&rel, &req, &mut s, &root, ports) {
            Some(Some(r)) => r,
            Some(None) => return,
            None => Resp::new(500, "Handler Not Emulated", "text/plain", format!("{rel} is not emulated").into_bytes()),
        }
    } else {
        let file = root.join(&rel);
        if !rel.contains("..") && file.is_file() {
            let mut body = std::fs::read(&file).unwrap_or_default();
            let mut headers: Vec<(String, String)> = vec![("Content-Type".into(), content_type(&file).into())];
            if let Ok(h) = std::fs::read_to_string(format!("{}.headers", file.display())) {
                for l in h.lines() {
                    if let Some((k, v)) = l.split_once(':') {
                        headers.retain(|(x, _)| !x.eq_ignore_ascii_case(k.trim()));
                        headers.push((k.trim().into(), v.trim().into()));
                    }
                }
            }
            let piped = req.get_str("pipe").unwrap_or_default();
            if rel.contains(".sub.") || piped.split('|').any(|p| p == "sub") {
                body = subst(&String::from_utf8_lossy(&body), &req, ports).into_bytes();
            }
            // The `trickle(dN)` pipe: an N-second delay before the body.
            for p in piped.split('|') {
                if let Some(d) = p.strip_prefix("trickle(d").and_then(|x| x.strip_suffix(')')).and_then(|x| x.parse::<f64>().ok()) {
                    sleep_ms((d * 1000.0) as u64);
                }
            }
            Resp { status: 200, reason: "OK".into(), headers, body }
        } else {
            Resp::new(404, "Not Found", "text/plain", b"not found".to_vec())
        }
    };
    write_resp(&mut s, resp);
}

// ------------------------------------------------------------------------------------------------ websockets

struct WsConn {
    s: TcpStream,
    dec: FrameDecoder,
    frag: Option<(u8, Vec<u8>)>,
}

enum In {
    Text(String),
    Binary(Vec<u8>),
    /// The client's close: its code and reason.
    Close(Option<(u16, String)>),
    Gone,
}

impl WsConn {
    fn send(&mut self, op: u8, payload: &[u8]) -> bool {
        self.s.write_all(&encode_frame(true, op, payload, None)).is_ok()
    }
    fn recv(&mut self) -> In {
        let mut tmp = [0u8; 16384];
        loop {
            match self.dec.next_frame() {
                Ok(Some(f)) => match f.opcode {
                    OP_PING => {
                        let _ = self.send(OP_PONG, &f.payload);
                    }
                    OP_PONG => {}
                    OP_CLOSE => {
                        let c = (f.payload.len() >= 2)
                            .then(|| (u16::from_be_bytes([f.payload[0], f.payload[1]]), String::from_utf8_lossy(&f.payload[2..]).into_owned()));
                        return In::Close(c);
                    }
                    OP_TEXT | OP_BINARY | OP_CONT => {
                        let (op, data) = match (f.opcode, self.frag.take()) {
                            (OP_CONT, Some((op, mut d))) => {
                                d.extend_from_slice(&f.payload);
                                (op, d)
                            }
                            (op, _) => (op, f.payload),
                        };
                        if !f.fin {
                            self.frag = Some((op, data));
                            continue;
                        }
                        return if op == OP_TEXT { In::Text(String::from_utf8_lossy(&data).into_owned()) } else { In::Binary(data) };
                    }
                    _ => return In::Gone,
                },
                Ok(None) => match self.s.read(&mut tmp) {
                    Ok(0) | Err(_) => return In::Gone,
                    Ok(n) => self.dec.push(&tmp[..n]),
                },
                Err(_) => return In::Gone,
            }
        }
    }
    /// The server-initiated closing handshake (what pywebsocket does when a handler returns).
    fn close(&mut self, code: Option<u16>, reason: &str) {
        let _ = self.send(OP_CLOSE, &close_payload(code, reason));
        let _ = self.s.set_read_timeout(Some(Duration::from_secs(3)));
        loop {
            match self.recv() {
                In::Close(_) | In::Gone => break,
                _ => {}
            }
        }
        let _ = self.s.shutdown(std::net::Shutdown::Both);
    }
    /// The passive closing handshake: answer the client's close (`echo` = with its code and reason;
    /// otherwise pywebsocket's default, 1000).
    fn passive(&mut self, got: Option<(u16, String)>, echo: bool) {
        let payload = if echo {
            match got {
                Some((c, r)) => close_payload(Some(c), &r),
                None => Vec::new(),
            }
        } else {
            close_payload(Some(1000), "")
        };
        let _ = self.send(OP_CLOSE, &payload);
        let _ = self.s.shutdown(std::net::Shutdown::Both);
    }
}

fn ws_conn(mut s: TcpStream) {
    let Some(req) = read_req(&mut s) else { return };
    let name = req.path.trim_start_matches('/').to_string();
    let Some(key) = req.header("sec-websocket-key").map(str::to_string) else {
        write_resp(&mut s, Resp::new(400, "Bad Request", "text/plain", Vec::new()));
        return;
    };
    if !WS_HANDLERS.contains(&name.as_str()) {
        write_resp(&mut s, Resp::new(404, "Not Found", "text/plain", Vec::new()));
        return;
    }
    let requested: Vec<String> = req.header("sec-websocket-protocol").map(|v| v.split(',').map(|p| p.trim().to_string()).collect()).unwrap_or_default();
    let origin = req.header("origin").unwrap_or("").to_string();
    let accept = accept_key(&key);
    match name.as_str() {
        "simple_handshake" => {
            let msg = format!(
                "HTTP/1.1 101 Switching Protocols:\r\nConnection: Upgrade\r\nUpgrade: WebSocket\r\nSet-Cookie: ws_test=test\r\nSec-WebSocket-Origin: {origin}\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            );
            let mut out = msg.into_bytes();
            out.extend(encode_frame(true, OP_CLOSE, &close_payload(Some(1001), "PASS"), None));
            let _ = s.write_all(&out);
            let mut tmp = [0u8; 64];
            let _ = s.set_read_timeout(Some(Duration::from_secs(2)));
            let _ = s.read(&mut tmp);
            let _ = s.shutdown(std::net::Shutdown::Both);
            return;
        }
        "wrong_accept_key" => {
            let _ = s.write_all(format!("HTTP/1.1 101 Switching Protocols:\r\nConnection: Upgrade\r\nUpgrade: WebSocket\r\nSec-WebSocket-Origin: {origin}\r\nSec-WebSocket-Accept: thisisawrongacceptkey\r\n\r\n").as_bytes());
            let _ = s.write_all(&encode_frame(true, OP_TEXT, b"test", None));
            sleep_ms(200);
            return;
        }
        "invalid" => {
            let _ = s.write_all(b"FOO BAR BAZ\r\n\r\n");
            sleep_ms(200);
            return;
        }
        "handshake_sleep_2" => sleep_ms(2000),
        _ => {}
    }
    let selected: Option<String> = match name.as_str() {
        "echo" => requested.iter().any(|p| p == "echo").then(|| "echo".to_string()),
        "protocol" => req.header("sec-websocket-protocol").map(str::to_string),
        "protocol_array" => requested.first().cloned(),
        "handshake_protocol" => Some("foobar".into()),
        _ => None,
    };
    let mut head = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n");
    if let Some(p) = &selected {
        head.push_str(&format!("Sec-WebSocket-Protocol: {p}\r\n"));
    }
    match name.as_str() {
        "set-cookie" => head.push_str(&format!("Set-Cookie: ws_test_{}=test; Path=/\r\n", req.query)),
        "set-cookie_http" => head.push_str(&format!("Set-Cookie: ws_test_{}=test; Path=/; HttpOnly\r\nSec-WebSocket-Origin: {origin}\r\n", req.query)),
        _ => {}
    }
    head.push_str("\r\n");
    if s.write_all(head.as_bytes()).is_err() {
        return;
    }
    let mut c = WsConn { s, dec: FrameDecoder::new(), frag: None };
    let query = req.target.split_once('?').map(|(_, q)| q.to_string()).unwrap_or_default();
    match name.as_str() {
        "echo_close_data" => loop {
            // Its echo is unreachable code in the handler (the send follows the `return`).
            match c.recv() {
                In::Text(t) if t == "Goodbye" => return c.close(Some(1000), ""),
                In::Close(got) => return c.passive(got, false),
                In::Gone => return,
                _ => {}
            }
        },
        "echo" => loop {
            match c.recv() {
                In::Text(t) => {
                    let _ = c.send(OP_TEXT, t.as_bytes());
                    if t == "Goodbye" {
                        return c.close(Some(1000), "");
                    }
                }
                In::Binary(b) => {
                    let _ = c.send(OP_BINARY, &b);
                }
                In::Close(got) => return c.passive(got, true),
                In::Gone => return,
            }
        },
        "echo_exit" => loop {
            match c.recv() {
                In::Text(t) if t == "Goodbye" => return c.close(Some(1000), ""),
                In::Close(got) => return c.passive(got, false),
                In::Gone => return,
                _ => {}
            }
        },
        "echo_raw" => loop {
            match c.recv() {
                In::Text(t) if t == "exit" => return c.close(Some(1000), ""),
                In::Binary(b) if b == b"exit" => return c.close(Some(1000), ""),
                In::Text(t) => {
                    let _ = c.s.write_all(t.as_bytes());
                }
                In::Binary(b) => {
                    let _ = c.s.write_all(&b);
                }
                In::Close(got) => return c.passive(got, false),
                In::Gone => return,
            }
        },
        "empty-message" => match c.recv() {
            In::Text(t) => {
                let _ = c.send(OP_TEXT, if t.is_empty() { b"pass" } else { b"fail" });
                c.close(Some(1000), "")
            }
            In::Binary(_) => {
                let _ = c.send(OP_TEXT, b"fail");
                c.close(Some(1000), "")
            }
            In::Close(got) => c.passive(got, false),
            In::Gone => {}
        },
        "echo-query" | "echo-query_v13" => {
            let _ = c.send(OP_TEXT, query.as_bytes());
            c.close(Some(1000), "")
        }
        "protocol" | "protocol_array" => {
            let _ = c.send(OP_TEXT, selected.unwrap_or_default().as_bytes());
            c.close(Some(1000), "")
        }
        "origin" => {
            let _ = c.send(OP_TEXT, origin.as_bytes());
            c.close(Some(1000), "")
        }
        "echo-cookie" => {
            let ck = req.header("cookie").map(str::to_string).unwrap_or_else(|| "(none)".into());
            let _ = c.send(OP_TEXT, ck.as_bytes());
            c.close(Some(1000), "")
        }
        "referrer" => {
            let r = req.header("referer").map(str::to_string).unwrap_or_else(|| "MISSING AS PER FETCH".into());
            let _ = c.send(OP_TEXT, r.as_bytes());
            c.close(Some(1000), "")
        }
        "set-cookie" | "set-cookie_http" => match c.recv() {
            In::Close(got) => c.passive(got, false),
            In::Gone => {}
            _ => c.close(Some(1000), ""),
        },
        "remote-close" => {
            if req.get("abrupt").is_some() {
                let _ = c.send(OP_PING, b"ping");
                let _ = c.s.shutdown(std::net::Shutdown::Both);
                return;
            }
            let code: Option<u16> = req.get_str("code").and_then(|v| v.parse().ok());
            let reason = req.get_str("reason").unwrap_or_default();
            c.close(code, if code.is_some() { &reason } else { "" })
        }
        "delayed-passive-close" => loop {
            match c.recv() {
                In::Close(got) => {
                    sleep_ms(1000);
                    return c.passive(got, true);
                }
                In::Gone => return,
                _ => {}
            }
        },
        // handshake_protocol, handshake_no_protocol, handshake_no_extensions, handshake_sleep_2: the
        // transfer step does nothing; the handler returning closes normally.
        _ => c.close(Some(1000), ""),
    }
}

/// Starts the HTTP and WebSocket servers over the checkout at `root`. AETHERWPT5 (SR144): the HTTP server
/// listens on two ports (`{{ports[http][0]}}`, `{{ports[http][1]}}`) and, for each, on every loopback address the
/// `sub` pipe names (`127.0.0.1` for `localhost` and the `www` hosts, `127.0.0.2`–`127.0.0.6`, and `[::1]` so a
/// resolver that lists it first connects at once) — one origin per (host, port).
pub fn start(root: PathBuf, report_js: &'static str) -> Ports {
    let http = TcpListener::bind("127.0.0.1:0").expect("bind http");
    let http2 = TcpListener::bind("127.0.0.1:0").expect("bind http2");
    let ws = TcpListener::bind("127.0.0.1:0").expect("bind ws");
    let ports = Ports { http: http.local_addr().unwrap().port(), http2: http2.local_addr().unwrap().port(), ws: ws.local_addr().unwrap().port() };
    let mut listeners = vec![http, http2];
    for addr in ["127.0.0.2", "127.0.0.3", "127.0.0.4", "127.0.0.5", "127.0.0.6", "[::1]"] {
        for port in [ports.http, ports.http2] {
            match TcpListener::bind(format!("{addr}:{port}")) {
                Ok(l) => listeners.push(l),
                Err(e) => eprintln!("wpt_net: cannot listen on {addr}:{port} ({e}); the pages that need that host will fail"),
            }
        }
    }
    for l in listeners {
        let root = root.clone();
        std::thread::spawn(move || {
            for s in l.incoming() {
                let Ok(s) = s else { continue };
                let root = root.clone();
                std::thread::spawn(move || http_conn(s, root, ports, report_js));
            }
        });
    }
    std::thread::spawn(move || {
        for s in ws.incoming() {
            let Ok(s) = s else { continue };
            let _ = s.set_nodelay(true);
            std::thread::spawn(move || ws_conn(s));
        }
    });
    ports
}
