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
//! Cross-origin hosts (`www`, `alt`) map to `localhost` — a different origin from `127.0.0.1` on the same
//! loopback — and wss/h2/https variants are not served (no TLS server).

use gneiss_pal::api::http::ws::{accept_key, close_payload, encode_frame, FrameDecoder, OP_BINARY, OP_CLOSE, OP_CONT, OP_PING, OP_PONG, OP_TEXT};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::time::Duration;

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

#[derive(Clone, Copy)]
pub struct Ports {
    pub http: u16,
    pub ws: u16,
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
            "host" | "domains[]" | "hosts[][]" | "location[hostname]" => Some("127.0.0.1".to_string()),
            k if k.starts_with("domains[") || k.starts_with("hosts[") => Some("localhost".to_string()),
            "ports[http][0]" | "ports[http][1]" => Some(ports.http.to_string()),
            "ports[ws][0]" => Some(ports.ws.to_string()),
            // No TLS server: an unused port, so wss/https variants fail fast (and are skipped by name).
            "ports[https][0]" | "ports[wss][0]" | "ports[h2][0]" | "ports[https][1]" => Some("1".into()),
            "location[scheme]" => Some("http".into()),
            "location[host]" | "location[server]" => Some(format!("127.0.0.1:{}", ports.http)),
            "location[port]" => Some(ports.http.to_string()),
            "location[path]" => Some(req.path.clone()),
            "location[query]" => Some(if req.query.is_empty() { String::new() } else { format!("?{}", req.query) }),
            "uuid()" => Some(format!("{:016x}-0000-4000-8000-000000000000", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64)),
            k => {
                if let Some(h) = k.strip_prefix("headers[").and_then(|x| x.strip_suffix(']')) {
                    Some(req.header(h.trim_matches('"')).unwrap_or("").to_string())
                } else if let Some(g) = k.strip_prefix("GET[").and_then(|x| x.strip_suffix(']')) {
                    Some(req.get_str(g.trim_matches('"')).unwrap_or_default())
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

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// The emulated Python handlers. `None` = not emulated.
fn py(rel: &str, req: &Req, s: &mut TcpStream) -> Option<Option<Resp>> {
    let es = "text/event-stream";
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
            let body = if req.get("full").is_some() { format!("http://127.0.0.1{}", req.target) } else { req.path.clone() };
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
        _ => return None,
    }))
}

fn http_conn(mut s: TcpStream, root: PathBuf, ports: Ports, report_js: &'static str) {
    let Some(req) = read_req(&mut s) else { return };
    let rel = req.path.trim_start_matches('/').to_string();
    let resp = if rel == "resources/testharnessreport.js" {
        Resp::new(200, "OK", "text/javascript", report_js.as_bytes().to_vec())
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
    } else if rel.ends_with(".py") {
        match py(&rel, &req, &mut s) {
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

/// Starts the HTTP and WebSocket servers over the checkout at `root`.
pub fn start(root: PathBuf, report_js: &'static str) -> Ports {
    let http = TcpListener::bind("127.0.0.1:0").expect("bind http");
    let ws = TcpListener::bind("127.0.0.1:0").expect("bind ws");
    let ports = Ports { http: http.local_addr().unwrap().port(), ws: ws.local_addr().unwrap().port() };
    std::thread::spawn(move || {
        for s in http.incoming() {
            let Ok(s) = s else { continue };
            let root = root.clone();
            std::thread::spawn(move || http_conn(s, root, ports, report_js));
        }
    });
    std::thread::spawn(move || {
        for s in ws.incoming() {
            let Ok(s) = s else { continue };
            let _ = s.set_nodelay(true);
            std::thread::spawn(move || ws_conn(s));
        }
    });
    ports
}
