//! AETHERNET (LEDGER SR70): the page-facing network against a local tokio server on 127.0.0.1:0 —
//! `WebSocket` (an echo endpoint, a close-code matrix, a fragmented message with a ping between its
//! fragments, a ping arriving while the client sends a 1 MiB message, a failed subprotocol handshake),
//! `EventSource` (a stream that drops once and expects `Last-Event-ID` on the reconnect, `retry:`
//! honoured), streaming `fetch` (a chunked body whose chunks leave 50 ms apart: the first `read()`
//! resolves before the last chunk is sent; `AbortController` mid-body and before the response),
//! `XMLHttpRequest` (headers, a JSON body, the readyState sequence, sync mode, abort, timeout, error),
//! and the Web Storage `storage` event between two documents of one origin (two engines, two threads).
//!
//! Each page runs on its own engine; the test drives its event loop (`Engine::tick`) on real time until
//! the page reports done.

use gneiss_pal::api::http::ws::{accept_key, close_payload, encode_frame, FrameDecoder, OP_BINARY, OP_CLOSE, OP_CONT, OP_PING, OP_PONG, OP_TEXT};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Default)]
struct State {
    chunks_sent: AtomicUsize,
    sse_conns: AtomicUsize,
    /// (connection index, Last-Event-ID header, ms since the first connection)
    sse_log: Mutex<Vec<(usize, Option<String>, u128)>>,
    sse_first: Mutex<Option<Instant>>,
    /// The close frames the server received, by the client's `?tag=`.
    ws_closes: Mutex<HashMap<String, Option<(u16, String)>>>,
    xhr_seen: Mutex<Vec<(String, String, String)>>,
}

struct Req {
    method: String,
    path: String,
    query: HashMap<String, String>,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

async fn read_req(s: &mut TcpStream) -> Option<(Req, Vec<u8>)> {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    let end = loop {
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&tmp[..n]);
    };
    let head = String::from_utf8_lossy(&buf[..end]).to_string();
    let mut lines = head.split("\r\n");
    let rl = lines.next()?;
    let mut parts = rl.split(' ');
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let (path, q) = target.split_once('?').unwrap_or((&target, ""));
    let query = q
        .split('&')
        .filter(|x| !x.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.to_string(), v.replace("%20", " "))
        })
        .collect();
    let mut headers = HashMap::new();
    for l in lines {
        if let Some((k, v)) = l.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let mut rest = buf[end..].to_vec();
    let len: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    while rest.len() < len {
        let n = s.read(&mut tmp).await.ok()?;
        if n == 0 {
            break;
        }
        rest.extend_from_slice(&tmp[..n]);
    }
    let body: Vec<u8> = rest.drain(..len.min(rest.len())).collect();
    Some((Req { method, path: path.to_string(), query, headers, body }, rest))
}

async fn respond(s: &mut TcpStream, status: &str, headers: &[(&str, &str)], body: &[u8]) {
    let mut h = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    let _ = s.write_all(h.as_bytes()).await;
    let _ = s.write_all(body).await;
    let _ = s.flush().await;
}

async fn handle(mut s: TcpStream, st: Arc<State>) {
    let Some((req, leftover)) = read_req(&mut s).await else { return };
    match req.path.as_str() {
        "/chunked" => {
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").await;
            for i in 0..4 {
                if i > 0 {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                let c = format!("part{i};");
                let _ = s.write_all(format!("{:x}\r\n{c}\r\n", c.len()).as_bytes()).await;
                let _ = s.flush().await;
                st.chunks_sent.fetch_add(1, Ordering::SeqCst);
            }
            let _ = s.write_all(b"0\r\n\r\n").await;
        }
        "/json" => respond(&mut s, "200 OK", &[("Content-Type", "application/json")], r#"{"a":[1,2],"b":"é"}"#.as_bytes()).await,
        "/xhr" => {
            let xreq = req.headers.get("x-req").cloned().unwrap_or_default();
            let body = String::from_utf8_lossy(&req.body).to_string();
            st.xhr_seen.lock().unwrap().push((req.method.clone(), xreq.clone(), req.headers.get("content-type").cloned().unwrap_or_default()));
            let j = format!(r#"{{"method":"{}","body":"{}","xreq":"{}"}}"#, req.method, body.replace('"', "'"), xreq);
            let h = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nX-Test: a\r\nX-Multi: 1\r\nX-Multi: 2\r\nSet-Cookie: s=1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                j.len()
            );
            let _ = s.write_all(h.as_bytes()).await;
            let _ = s.write_all(j.as_bytes()).await;
        }
        "/slow" => {
            tokio::time::sleep(Duration::from_millis(1500)).await;
            respond(&mut s, "200 OK", &[("Content-Type", "text/plain")], b"slow").await;
        }
        "/status" => {
            let code = req.query.get("code").cloned().unwrap_or_else(|| "404".into());
            respond(&mut s, &format!("{code} Custom Reason"), &[("Content-Type", "text/plain")], b"x").await;
        }
        "/sse" => {
            let n = st.sse_conns.fetch_add(1, Ordering::SeqCst);
            let t0 = *st.sse_first.lock().unwrap().get_or_insert_with(Instant::now);
            st.sse_log.lock().unwrap().push((n, req.headers.get("last-event-id").cloned(), t0.elapsed().as_millis()));
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n").await;
            if n == 0 {
                let _ = s.write_all(b"retry: 100\nid: 1\ndata: one\n\n").await;
                let _ = s.flush().await;
                tokio::time::sleep(Duration::from_millis(30)).await;
                let _ = s.write_all(b": comment\r\nevent: named\r\ndata: two\r\ndata: lines\r\n\r\n").await;
                let _ = s.flush().await;
                tokio::time::sleep(Duration::from_millis(30)).await;
                // Drop the connection: the client must reconnect with Last-Event-ID: 1.
            } else {
                let id = req.headers.get("last-event-id").cloned().unwrap_or_default();
                let _ = s.write_all(format!("data: last={id}\n\n").as_bytes()).await;
                let _ = s.flush().await;
                tokio::time::sleep(Duration::from_millis(3000)).await;
            }
        }
        "/sse-404" => respond(&mut s, "404 Not Found", &[("Content-Type", "text/event-stream")], b"data: x\n\n").await,
        "/ws" => websocket(s, req, leftover, st).await,
        _ => respond(&mut s, "200 OK", &[("Content-Type", "text/html")], b"<!doctype html><title>t</title><body>x</body>").await,
    }
}

async fn ws_send(s: &mut TcpStream, fin: bool, op: u8, payload: &[u8]) {
    let _ = s.write_all(&encode_frame(fin, op, payload, None)).await;
    let _ = s.flush().await;
}

/// A WebSocket endpoint (RFC 6455 server side, unmasked frames out, masked frames in): echo, plus
/// commands the page sends as text — `frag`, `ping-me` (then a large binary), `close:<code>:<reason>`,
/// `close-empty`, `drop`.
async fn websocket(mut s: TcpStream, req: Req, leftover: Vec<u8>, st: Arc<State>) {
    let Some(key) = req.headers.get("sec-websocket-key").cloned() else {
        respond(&mut s, "400 Bad Request", &[], b"").await;
        return;
    };
    let tag = req.query.get("tag").cloned().unwrap_or_default();
    let offered: Vec<String> = req.headers.get("sec-websocket-protocol").map(|v| v.split(',').map(|x| x.trim().to_string()).collect()).unwrap_or_default();
    let mut head = format!("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {}\r\n", accept_key(&key));
    if req.query.get("proto").map(String::as_str) != Some("none") {
        if let Some(p) = offered.first() {
            head.push_str(&format!("Sec-WebSocket-Protocol: {p}\r\n"));
        }
    }
    head.push_str("\r\n");
    let _ = s.write_all(head.as_bytes()).await;
    let mut dec = FrameDecoder::new();
    dec.push(&leftover);
    let mut tmp = vec![0u8; 64 * 1024];
    let mut frag: Option<(u8, Vec<u8>)> = None;
    let mut await_pong_p1 = false;
    // ping-me: (ping sent mid-transfer, pong seen, big message length)
    let mut big: Option<(bool, bool, Option<usize>)> = None;
    loop {
        let f = match dec.next_frame() {
            Ok(Some(f)) => f,
            Ok(None) => {
                let n = match s.read(&mut tmp).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => n,
                };
                dec.push(&tmp[..n]);
                if let Some((sent, _, None)) = &mut big {
                    // The first bytes of the big message are arriving: ping while the client is still sending it.
                    if !*sent {
                        ws_send(&mut s, true, OP_PING, b"x").await;
                        *sent = true;
                    }
                }
                continue;
            }
            Err(_) => return,
        };
        assert!(f.mask.is_some(), "client frames must be masked");
        match f.opcode {
            OP_PING => ws_send(&mut s, true, OP_PONG, &f.payload).await,
            OP_PONG => {
                if f.payload == b"p1" && await_pong_p1 {
                    await_pong_p1 = false;
                    ws_send(&mut s, true, OP_TEXT, b"pong:p1").await;
                }
                if f.payload == b"x" {
                    if let Some(b) = &mut big {
                        b.1 = true;
                    }
                }
            }
            OP_CLOSE => {
                let parsed = if f.payload.len() >= 2 {
                    Some((u16::from_be_bytes([f.payload[0], f.payload[1]]), String::from_utf8_lossy(&f.payload[2..]).to_string()))
                } else {
                    None
                };
                st.ws_closes.lock().unwrap().insert(tag.clone(), parsed);
                ws_send(&mut s, true, OP_CLOSE, &f.payload).await;
                let _ = s.shutdown().await;
                return;
            }
            OP_TEXT | OP_BINARY | OP_CONT => {
                let (op, data) = match (f.opcode, frag.take()) {
                    (OP_CONT, Some((op, mut d))) => {
                        d.extend_from_slice(&f.payload);
                        (op, d)
                    }
                    (op, _) => (op, f.payload.clone()),
                };
                if !f.fin {
                    frag = Some((op, data));
                    continue;
                }
                if op == OP_BINARY {
                    if let Some(b) = &mut big {
                        b.2 = Some(data.len());
                    } else {
                        ws_send(&mut s, true, OP_BINARY, &data).await;
                    }
                } else {
                    let t = String::from_utf8(data).unwrap();
                    if t == "frag" {
                        ws_send(&mut s, false, OP_TEXT, b"fr").await;
                        ws_send(&mut s, true, OP_PING, b"p1").await;
                        ws_send(&mut s, false, OP_CONT, b"ag-").await;
                        ws_send(&mut s, true, OP_CONT, "m\u{e9}nt".as_bytes()).await;
                        await_pong_p1 = true;
                    } else if t == "ping-me" {
                        big = Some((false, false, None));
                    } else if let Some(rest) = t.strip_prefix("close:") {
                        let (c, r) = rest.split_once(':').unwrap();
                        ws_send(&mut s, true, OP_CLOSE, &close_payload(Some(c.parse().unwrap()), r)).await;
                    } else if t == "close-empty" {
                        ws_send(&mut s, true, OP_CLOSE, &[]).await;
                    } else if t == "drop" {
                        let _ = s.shutdown().await;
                        return;
                    } else {
                        ws_send(&mut s, true, OP_TEXT, t.as_bytes()).await;
                    }
                }
            }
            _ => {}
        }
        if let Some((_, true, Some(n))) = big {
            ws_send(&mut s, true, OP_TEXT, format!("big:{n}:pong").as_bytes()).await;
            big = None;
        }
    }
}

fn server() -> (u16, Arc<State>) {
    let st = Arc::new(State::default());
    let (tx, rx) = std::sync::mpsc::channel();
    let st2 = st.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async move {
            let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(l.local_addr().unwrap().port()).unwrap();
            loop {
                let Ok((c, _)) = l.accept().await else { continue };
                let _ = c.set_nodelay(true);
                tokio::spawn(handle(c, st2.clone()));
            }
        });
    });
    (rx.recv().unwrap(), st)
}

fn page(port: u16, script: &str) -> aether::AetherEngine {
    let mut engine = aether::AetherEngine::new();
    let html = format!("<!doctype html><html><body><div id=x></div><script>{script}</script></body></html>");
    engine.load_html_styled(&format!("http://127.0.0.1:{port}/page"), &html, &[], true);
    engine
}

/// Drives the page's event loop on real time until `cond` (a JS expression) is truthy; answers `out`.
fn run_until(engine: &mut aether::AetherEngine, cond: &str, out: &str, secs: u64) -> String {
    let js = engine.js_engine.as_mut().expect("engine");
    let end = Instant::now() + Duration::from_secs(secs);
    loop {
        js.tick();
        if js.eval_string(&format!("!!({cond})")).unwrap_or_default() == "true" {
            return js.eval_string(out).unwrap_or_else(|e| format!("<error {e}>"));
        }
        if Instant::now() > end {
            let errs = aether::js::reported_errors();
            panic!("timed out waiting for {cond}; {out} = {:?}; errors: {errs:?}", js.eval_string(out));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn websocket_echo_fragments_and_ping_during_send() {
    let (port, _st) = server();
    let mut e = page(
        port,
        &format!(
            r#"
        var log = [];
        var ws = new WebSocket('ws://127.0.0.1:{port}/ws?tag=echo', ['chat', 'superchat']);
        log.push('state' + ws.readyState + ':' + ws.url + ':' + ws.binaryType);
        ws.binaryType = 'nonsense';
        ws.binaryType = 'arraybuffer';
        ws.onopen = function (ev) {{
            log.push('open:' + ws.protocol + ':' + ws.readyState + ':' + ws.extensions + ':' + ev.isTrusted);
            ws.send('hello');
            ws.send(new Uint8Array([1, 2, 3]));
            ws.send(new Uint16Array([0x0201]).buffer);
            ws.send('frag');
            ws.send('ping-me');
            var big = new Uint8Array(1048576);
            ws.send(big);
            log.push('buffered>0:' + (ws.bufferedAmount >= 1048576));
        }};
        ws.addEventListener('message', function (ev) {{
            var d = ev.data;
            log.push(typeof d === 'string' ? 't:' + d : 'b:' + (d instanceof ArrayBuffer) + ':' + new Uint8Array(d).join(','));
            if (typeof d === 'string' && d.indexOf('big:') === 0) {{ ws.close(3000, 'bye'); log.push('closing' + ws.readyState); }}
        }});
        ws.onclose = function (ev) {{
            log.push('close:' + ev.code + ':' + ev.reason + ':' + ev.wasClean + ':' + ws.readyState + ':' + ws.bufferedAmount + ':' + (ev instanceof CloseEvent));
            window.done = log.join('|');
        }};
        ws.onerror = function () {{ log.push('error'); }};
    "#
        ),
    );
    let got = run_until(&mut e, "window.done", "window.done", 20);
    assert_eq!(
        got,
        format!(
            "state0:ws://127.0.0.1:{port}/ws?tag=echo:blob|open:chat:1::true|buffered>0:true|t:hello|b:true:1,2,3|b:true:1,2|t:frag-m\u{e9}nt|t:pong:p1|t:big:1048576:pong|closing2|close:3000:bye:true:3:0:true"
        ),
    );
}

#[test]
fn websocket_close_code_matrix_and_failures() {
    let (port, st) = server();
    let mut e = page(
        port,
        &format!(
            r#"
        var R = {{}};
        var u = function (tag, q) {{ return 'ws://127.0.0.1:{port}/ws?tag=' + tag + (q || ''); }};
        var track = function (tag, ws, onopen) {{
            var errs = 0;
            ws.onerror = function () {{ errs++; }};
            ws.onopen = function () {{ onopen(ws); }};
            ws.onclose = function (e) {{ R[tag] = e.code + ':' + e.reason + ':' + e.wasClean + ':err' + errs; }};
        }};
        track('c1000', new WebSocket(u('c1000')), function (ws) {{ ws.close(1000, 'done'); }});
        track('cnone', new WebSocket(u('cnone')), function (ws) {{ ws.close(); }});
        track('c4999', new WebSocket(u('c4999')), function (ws) {{ ws.close(4999, 'x'); }});
        track('creason', new WebSocket(u('creason')), function (ws) {{ ws.close(undefined, 'only reason'); }});
        track('s1001', new WebSocket(u('s1001')), function (ws) {{ ws.send('close:1001:going'); }});
        track('s4000', new WebSocket(u('s4000')), function (ws) {{ ws.send('close:4000:app'); }});
        track('sempty', new WebSocket(u('sempty')), function (ws) {{ ws.send('close-empty'); }});
        track('drop', new WebSocket(u('drop')), function (ws) {{ ws.send('drop'); }});
        track('noproto', new WebSocket(u('noproto', '&proto=none'), 'chat'), function () {{ R.noproto_opened = true; }});
        track('refused', new WebSocket('ws://127.0.0.1:1/'), function () {{}});
        var connecting = new WebSocket(u('abort'));
        track('abort', connecting, function () {{}});
        connecting.close();
        R.abortState = connecting.readyState;
        var ex = function (f) {{ try {{ f(); return 'none'; }} catch (e) {{ return e.name; }} }};
        var probe = new WebSocket(u('probe'));
        R.sendEarly = ex(function () {{ probe.send('x'); }});
        probe.onopen = function () {{
            R.invalid = [1001, 2999, 5000, 0, 65536 + 1000].map(function (c) {{ return ex(function () {{ probe.close(c); }}); }}).join(',');
            R.longReason = ex(function () {{ probe.close(1000, new Array(125).join('a')); }});
            R.okReason = ex(function () {{ probe.close(1000, new Array(124).join('a')); }});
        }};
        probe.onclose = function (e) {{ R.probe = e.code + ':' + e.reason.length; }};
        R.ctor = [
            ex(function () {{ new WebSocket('ftp://127.0.0.1/'); }}),
            ex(function () {{ new WebSocket('ws://127.0.0.1/#frag'); }}),
            ex(function () {{ new WebSocket('ws://127.0.0.1/', ['a', 'A']); }}),
            ex(function () {{ new WebSocket('ws://127.0.0.1/', 'has space'); }}),
            ex(function () {{ new WebSocket('ws://[bad'); }}),
            ex(function () {{ new WebSocket(); }}),
        ].join(',');
        var http = new WebSocket('http://127.0.0.1:{port}/ws?tag=http');
        R.httpUrl = http.url;
        http.onopen = function () {{ http.close(); }};
        http.onclose = function (e) {{ R.http = e.code; }};
    "#
        ),
    );
    let keys = ["c1000", "cnone", "c4999", "creason", "s1001", "s4000", "sempty", "drop", "noproto", "refused", "abort", "probe", "http"];
    let cond = keys.iter().map(|k| format!("R.{k} !== undefined")).collect::<Vec<_>>().join(" && ");
    let got = run_until(&mut e, &cond, "JSON.stringify(R)", 30);
    let v: serde_json::Value = serde_json::from_str(&got).unwrap();
    let g = |k: &str| v[k].as_str().map(str::to_string).unwrap_or_else(|| v[k].to_string());
    assert_eq!(g("c1000"), "1000:done:true:err0");
    assert_eq!(g("cnone"), "1005::true:err0", "close() with no code: the server echoes an empty close");
    assert_eq!(g("c4999"), "4999:x:true:err0");
    assert_eq!(g("creason"), "1000:only reason:true:err0", "a reason with no code sends 1000");
    assert_eq!(g("s1001"), "1001:going:true:err0");
    assert_eq!(g("s4000"), "4000:app:true:err0");
    assert_eq!(g("sempty"), "1005::true:err0");
    assert_eq!(g("drop"), "1006::false:err1", "an abrupt TCP close: error then close 1006");
    assert_eq!(g("noproto"), "1006::false:err1", "a subprotocol was requested and none selected: failed");
    assert!(v["noproto_opened"].is_null());
    assert_eq!(g("refused"), "1006::false:err1");
    assert_eq!(g("abort"), "1006::false:err1", "close() while CONNECTING fails the connection");
    assert_eq!(g("abortState"), "2");
    assert_eq!(g("sendEarly"), "InvalidStateError");
    assert_eq!(g("invalid"), "InvalidAccessError,InvalidAccessError,InvalidAccessError,InvalidAccessError,InvalidAccessError");
    assert_eq!(g("longReason"), "SyntaxError");
    assert_eq!(g("okReason"), "none");
    assert_eq!(g("probe"), "1000:123");
    assert_eq!(g("ctor"), "SyntaxError,SyntaxError,SyntaxError,SyntaxError,SyntaxError,TypeError");
    assert_eq!(g("httpUrl"), format!("ws://127.0.0.1:{port}/ws?tag=http"));
    assert_eq!(g("http"), "1005");
    // What the server received on the wire.
    let closes = st.ws_closes.lock().unwrap();
    assert_eq!(closes["c1000"], Some((1000, "done".into())));
    assert_eq!(closes["cnone"], None);
    assert_eq!(closes["c4999"], Some((4999, "x".into())));
    assert_eq!(closes["creason"], Some((1000, "only reason".into())));
    assert_eq!(closes["s1001"], Some((1001, String::new())), "the client echoes the server's code");
    assert_eq!(closes["s4000"], Some((4000, String::new())));
}

#[test]
fn eventsource_reconnects_with_last_event_id() {
    let (port, st) = server();
    let mut e = page(
        port,
        r#"
        var log = [];
        var es = new EventSource('/sse');
        log.push('init' + es.readyState + ':' + es.url.replace(/^http:\/\/[^\/]+/, '') + ':' + es.withCredentials);
        es.onopen = function () { log.push('open' + es.readyState); };
        es.onmessage = function (e) {
            log.push('m:' + e.data + ':' + e.lastEventId + ':' + e.origin.replace(/:\d+$/, '') + ':' + e.isTrusted);
            if (e.data.indexOf('last=') === 0) { es.close(); log.push('closed' + es.readyState); window.done = log.join('|'); }
        };
        es.addEventListener('named', function (e) { log.push('n:' + e.data.replace('\n', '/') + ':' + e.lastEventId); });
        es.onerror = function () { log.push('err' + es.readyState); };
        var bad = new EventSource('/sse-404');
        bad.onerror = function () { window.bad = 'err' + bad.readyState; };
        var bogus;
        try { new EventSource('http://[x'); } catch (e) { bogus = e.name; }
        window.bogus = bogus;
    "#,
    );
    let got = run_until(&mut e, "window.done && window.bad", "window.done + '#' + window.bad + '#' + window.bogus", 20);
    assert_eq!(
        got,
        "init0:/sse:false|open1|m:one:1:http://127.0.0.1:true|n:two/lines:1|err0|open1|m:last=1:1:http://127.0.0.1:true|closed2#err2#SyntaxError"
    );
    let log = st.sse_log.lock().unwrap().clone();
    assert_eq!(log.len(), 2, "one drop, one reconnect: {log:?}");
    assert_eq!(log[0].1, None);
    assert_eq!(log[1].1.as_deref(), Some("1"), "the reconnect carries Last-Event-ID");
    assert!(log[1].2 < 1500, "retry: 100 shortened the 3 s default reconnection time: {}ms", log[1].2);
}

#[test]
fn fetch_streams_chunks_before_eof() {
    let (port, st) = server();
    let mut e = page(
        port,
        r#"
        var parts = [];
        fetch('/chunked').then(function (r) {
            window.head = r.status + ':' + r.ok + ':' + r.headers.get('content-type') + ':' + (r.body instanceof ReadableStream) + ':' + r.bodyUsed;
            var reader = r.body.getReader();
            window.locked = r.body.locked;
            var dec = new TextDecoder();
            var pump = function () {
                return reader.read().then(function (res) {
                    if (res.done) { window.done = parts.join('') + ':' + r.bodyUsed; return; }
                    if (window.firstRead === undefined) { window.firstRead = dec.decode(res.value) + ':' + (res.value instanceof Uint8Array); }
                    parts.push(dec.decode(res.value));
                    return pump();
                });
            };
            return pump();
        });
    "#,
    );
    // The first read resolves while the server still has chunks to send (they leave 50 ms apart).
    let first = run_until(&mut e, "window.firstRead !== undefined", "window.firstRead", 10);
    let sent_at_first_read = st.chunks_sent.load(Ordering::SeqCst);
    assert_eq!(first, "part0;:true");
    assert!(sent_at_first_read < 4, "the first read() resolved before the last chunk was sent ({sent_at_first_read} of 4 sent)");
    let done = run_until(&mut e, "window.done", "window.head + '|' + window.locked + '|' + window.done", 10);
    assert_eq!(done, "200:true:text/plain:true:false|true|part0;part1;part2;part3;:true");
}

#[test]
fn fetch_body_mixins_abort_and_errors() {
    let (port, _st) = server();
    let mut e = page(
        port,
        &format!(
            r#"
        var R = {{}};
        fetch('/json').then(function (r) {{ return r.json(); }}).then(function (j) {{ R.json = JSON.stringify(j); }});
        fetch('/json').then(function (r) {{ return r.arrayBuffer(); }}).then(function (b) {{ R.ab = (b instanceof ArrayBuffer) + ':' + b.byteLength; }});
        fetch('/json').then(function (r) {{
            var c = r.clone();
            return r.text().then(function (t) {{
                R.used = r.bodyUsed;
                return r.text().then(function () {{ R.twice = 'no'; }}, function (e) {{ R.twice = e.name; }}).then(function () {{ return c.text(); }});
            }});
        }}).then(function (t) {{ R.clone = t.length; }});
        fetch('/status?code=404').then(function (r) {{ R.s404 = r.status + ':' + r.ok + ':' + r.statusText; }});
        fetch('http://127.0.0.1:1/').then(function () {{ R.refused = 'resolved'; }}, function (e) {{ R.refused = e.name; }});
        var ac = new AbortController();
        fetch('/slow', {{ signal: ac.signal }}).then(function () {{ R.abortEarly = 'resolved'; }}, function (e) {{ R.abortEarly = e.name + ':' + (e instanceof DOMException); }});
        setTimeout(function () {{ ac.abort(); }}, 20);
        var ac2 = new AbortController();
        fetch('/chunked', {{ signal: ac2.signal }}).then(function (r) {{
            var rd = r.body.getReader();
            return rd.read().then(function (first) {{
                ac2.abort();
                return rd.read().then(function () {{ R.abortMid = 'read'; }}, function (e) {{ R.abortMid = new TextDecoder().decode(first.value) + ':' + e.name; }});
            }});
        }});
        fetch('/chunked').then(function (r) {{ return r.body.cancel('enough').then(function () {{ R.cancel = r.bodyUsed; }}); }});
        fetch('/xhr', {{ method: 'POST', body: 'a=1', headers: {{ 'X-Req': 'f' }} }}).then(function (r) {{ return r.json(); }}).then(function (j) {{ R.post = j.method + ':' + j.body + ':' + j.xreq; }});
        R.sigAbort = ex(function () {{ new Request('/x', {{ signal: AbortSignal.abort() }}).signal.throwIfAborted(); }});
        function ex(f) {{ try {{ f(); return 'none'; }} catch (e) {{ return e.name; }} }}
        var rs = new ReadableStream({{ start: function (c) {{ c.enqueue('a'); c.enqueue('b'); c.close(); }} }});
        var rd = rs.getReader();
        R.lockedErr = ex(function () {{ rs.getReader(); }});
        rd.read().then(function (a) {{ return rd.read().then(function (b) {{ return rd.read().then(function (c) {{ R.stream = a.value + b.value + c.done; }}); }}); }});
        new Response('hi', {{ status: 201, headers: {{ 'X-A': 'b' }} }}).text().then(function (t) {{ R.ctor = t; }});
    "#
        ),
    );
    let keys = ["json", "ab", "used", "twice", "clone", "s404", "refused", "abortEarly", "abortMid", "cancel", "post", "stream", "ctor"];
    let cond = keys.iter().map(|k| format!("R.{k} !== undefined")).collect::<Vec<_>>().join(" && ");
    let got = run_until(&mut e, &cond, "JSON.stringify(R)", 20);
    let v: serde_json::Value = serde_json::from_str(&got).unwrap();
    let g = |k: &str| v[k].as_str().map(str::to_string).unwrap_or_else(|| v[k].to_string());
    assert_eq!(g("json"), r#"{"a":[1,2],"b":"é"}"#);
    assert_eq!(g("ab"), "true:20", "20 bytes: é is two");
    assert_eq!(g("used"), "true");
    assert_eq!(g("twice"), "TypeError");
    assert_eq!(g("clone"), "19");
    assert_eq!(g("s404"), "404:false:Custom Reason");
    assert_eq!(g("refused"), "TypeError");
    assert_eq!(g("abortEarly"), "AbortError:true");
    assert_eq!(g("abortMid"), "part0;:AbortError");
    assert_eq!(g("cancel"), "true");
    assert_eq!(g("post"), "POST:a=1:f");
    assert_eq!(g("sigAbort"), "AbortError");
    assert_eq!(g("lockedErr"), "TypeError");
    assert_eq!(g("stream"), "abtrue");
    assert_eq!(g("ctor"), "hi");
}

#[test]
fn xhr_states_headers_json_sync_abort_timeout() {
    let (port, st) = server();
    let mut e = page(
        port,
        r#"
        var R = {};
        var x = new XMLHttpRequest();
        var states = [];
        x.onreadystatechange = function () { states.push(x.readyState); };
        x.open('GET', '/xhr?q=1');
        x.setRequestHeader('X-Req', 'abc');
        x.setRequestHeader('Cookie', 'forbidden');
        x.responseType = 'json';
        var events = [];
        ['loadstart', 'progress', 'load', 'loadend', 'error', 'abort'].forEach(function (t) {
            x.addEventListener(t, function (ev) { events.push(t + (t === 'loadend' ? ':' + ev.loaded + ':' + ev.total + ':' + ev.lengthComputable : '')); });
        });
        x.onload = function () {
            R.get = [x.status, x.statusText, x.response.method, x.response.xreq, x.getResponseHeader('x-test'),
                     x.getResponseHeader('X-MULTI'), String(x.getResponseHeader('set-cookie')), x.responseURL.replace(/^http:\/\/[^\/]+/, '')].join('|');
            R.all = x.getAllResponseHeaders().replace(/\r\n/g, '\\n');
        };
        x.onloadend = function () { R.states = states.join(','); R.events = events.join(','); };
        x.send();
        R.afterSend = x.readyState;

        var p = new XMLHttpRequest();
        p.open('POST', '/xhr');
        p.onload = function () { R.post = p.responseText; };
        p.send('x=1');

        var s = new XMLHttpRequest();
        var sstates = [];
        s.onreadystatechange = function () { sstates.push(s.readyState); };
        s.open('GET', '/json', false);
        s.send();
        R.sync = s.status + ':' + s.responseText + ':' + sstates.join(',');

        var b = new XMLHttpRequest();
        b.open('GET', '/json');
        b.responseType = 'arraybuffer';
        b.onload = function () { R.ab = (b.response instanceof ArrayBuffer) + ':' + b.response.byteLength + ':' + (b.response === b.response); };
        b.send();

        var a = new XMLHttpRequest();
        var aev = [];
        a.open('GET', '/slow');
        a.onabort = function () { aev.push('abort' + a.readyState); };
        a.onloadend = function () { aev.push('loadend'); };
        a.onreadystatechange = function () { aev.push('rs' + a.readyState); };
        a.send();
        setTimeout(function () { a.abort(); R.abort = aev.join(',') + ':' + a.readyState + ':' + a.status; }, 10);

        var t = new XMLHttpRequest();
        t.open('GET', '/slow');
        t.timeout = 50;
        t.ontimeout = function (ev) { R.timeout = 'timeout' + t.readyState + ':' + t.status + ':' + (ev instanceof ProgressEvent); };
        t.send();

        var er = new XMLHttpRequest();
        er.open('GET', 'http://127.0.0.1:1/');
        er.onerror = function () { R.error = 'error' + er.readyState + ':' + er.status; };
        er.send();

        var ex = function (f) { try { f(); return 'none'; } catch (e) { return e.name; } };
        var q = new XMLHttpRequest();
        R.excs = [
            ex(function () { q.send(); }),
            ex(function () { q.setRequestHeader('a', 'b'); }),
            ex(function () { q.open('GET', 'http://[bad'); }),
            ex(function () { q.open('BAD METHOD', '/'); }),
            ex(function () { q.open('TRACE', '/'); }),
            ex(function () { q.open('GET', '/json', false); q.responseType = 'json'; }),
        ].join(',');
    "#,
    );
    let keys = ["get", "states", "post", "sync", "ab", "abort", "timeout", "error"];
    let cond = keys.iter().map(|k| format!("R.{k} !== undefined")).collect::<Vec<_>>().join(" && ");
    let got = run_until(&mut e, &cond, "JSON.stringify(R)", 20);
    let v: serde_json::Value = serde_json::from_str(&got).unwrap();
    let g = |k: &str| v[k].as_str().map(str::to_string).unwrap_or_else(|| v[k].to_string());
    assert_eq!(g("get"), "200|OK|GET|abc|a|1, 2|null|/xhr?q=1");
    assert_eq!(g("all"), "connection: close\\ncontent-length: 39\\ncontent-type: application/json\\nx-multi: 1, 2\\nx-test: a\\n");
    assert_eq!(g("states"), "1,2,3,4");
    assert_eq!(g("events"), "loadstart,progress,progress,load,loadend:39:39:true");
    assert_eq!(g("afterSend"), "1");
    assert_eq!(g("post"), r#"{"method":"POST","body":"x=1","xreq":""}"#);
    assert_eq!(g("sync"), r#"200:{"a":[1,2],"b":"é"}:1,4"#);
    assert_eq!(g("ab"), "true:20:true");
    assert_eq!(g("abort"), "rs4,abort4,loadend:0:0");
    assert_eq!(g("timeout"), "timeout4:0:true");
    assert_eq!(g("error"), "error4:0");
    assert_eq!(g("excs"), "InvalidStateError,InvalidStateError,SyntaxError,SyntaxError,SecurityError,InvalidAccessError");
    let seen = st.xhr_seen.lock().unwrap();
    let get = seen.iter().find(|s| s.0 == "GET").unwrap();
    assert_eq!(get.1, "abc", "setRequestHeader reached the wire");
    let post = seen.iter().find(|s| s.0 == "POST").unwrap();
    assert_eq!(post.2, "text/plain;charset=UTF-8", "a string body is sent as UTF-8 text");
}

#[test]
fn storage_event_reaches_the_other_documents_of_the_origin() {
    let (port, _st) = server();
    let origin = format!("http://127.0.0.1:{port}");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    // Document B: listens.
    let o2 = origin.clone();
    let b = std::thread::spawn(move || {
        let mut e = aether::AetherEngine::new();
        e.load_html_styled(
            &format!("{o2}/b"),
            r#"<!doctype html><body><script>
                var got = [];
                window.addEventListener('storage', function (ev) {
                    got.push([ev.key, ev.oldValue, ev.newValue, ev.url.replace(/^http:\/\/[^\/]+/, ''), ev.storageArea === localStorage,
                              ev instanceof StorageEvent, ev.isTrusted].join(','));
                });
                window.onstorage = function () { got.push('handler'); };
            </script></body>"#,
            &[],
            true,
        );
        ready_tx.send(()).unwrap();
        run_until(
            &mut e,
            "got.filter(function (g) { return g !== 'handler'; }).length >= 7",
            "got.join('|') + '#' + localStorage.getItem('k') + '#' + localStorage.length",
            20,
        )
    });
    // Document C: another origin, must hear nothing.
    let c = std::thread::spawn(move || {
        let mut e = aether::AetherEngine::new();
        e.load_html_styled(
            "http://other.invalid/c",
            "<!doctype html><body><script>var heard = 0; window.addEventListener('storage', function () { heard++; });</script></body>",
            &[],
            true,
        );
        let js = e.js_engine.as_mut().unwrap();
        go_rx.recv().unwrap();
        for _ in 0..100 {
            js.tick();
            std::thread::sleep(Duration::from_millis(2));
        }
        js.eval_string("heard + ':' + localStorage.length").unwrap()
    });
    ready_rx.recv().unwrap();
    // Document A: writes.
    let mut a = aether::AetherEngine::new();
    a.load_html_styled(
        &format!("{origin}/a"),
        r#"<!doctype html><body><script>
            var self_heard = 0;
            window.addEventListener('storage', function () { self_heard++; });
            localStorage.clear();
            localStorage.setItem('k', 'v1');
            localStorage.k = 'v2';
            localStorage.setItem('k', 'v2');
            localStorage.setItem('gone', 'x');
            delete localStorage.gone;
            localStorage.setItem('z', '1');
            localStorage.clear();
            localStorage.setItem('k', 'final');
            var named = localStorage.k + ':' + ('k' in localStorage) + ':' + Object.keys(localStorage).join(',') + ':' + (localStorage instanceof Storage);
            var quota;
            try { localStorage.setItem('big', new Array(5 * 1024 * 1024 + 2).join('x')); quota = 'stored'; } catch (e) { quota = e.name; }
        </script></body>"#,
        &[],
        true,
    );
    let got_b = b.join().unwrap();
    go_tx.send(()).unwrap();
    let js = a.js_engine.as_mut().unwrap();
    for _ in 0..50 {
        js.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
    let a_res = js.eval_string("self_heard + '#' + named + '#' + quota").unwrap();
    assert_eq!(a_res, "0#final:true:k:true#QuotaExceededError", "the writer does not hear its own writes");
    // setItem k v1; k v2 (via named property); the second v2 is no change (no event); gone x; remove gone;
    // z 1; clear; k final — seven events at B (each also runs onstorage).
    let (events, rest) = got_b.split_once('#').unwrap();
    let listener: Vec<&str> = events.split('|').filter(|e| *e != "handler").collect();
    assert_eq!(
        listener[..7],
        [
            "k,,v1,/a,true,true,true",
            "k,v1,v2,/a,true,true,true",
            "gone,,x,/a,true,true,true",
            "gone,x,,/a,true,true,true",
            "z,,1,/a,true,true,true",
            ",,,/a,true,true,true",
            "k,,final,/a,true,true,true",
        ]
    );
    assert!(events.contains("handler"), "onstorage fires too");
    assert_eq!(rest, "final#1", "one localStorage area per origin, shared by the process's documents");
    let c_res = c.join().unwrap();
    assert_eq!(c_res, "0:0", "another origin hears nothing and sees an empty area");
}
