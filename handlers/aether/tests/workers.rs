//! AETHERWORKERS (LEDGER SR123): dedicated workers, MessageChannel/MessagePort and requestIdleCallback, one
//! host test per rule, each on its own page (its own document, its own inbox):
//!
//! 1. a worker echoes a message (DedicatedWorkerGlobalScope, `name`, MessageEvent origin "" and no ports);
//! 2. an ArrayBuffer in the transfer list is detached on the sender, both ways;
//! 3. an uncaught worker error fires an ErrorEvent at the Worker (message, filename, a SyntaxError's lineno),
//!    a worker `onerror` returning true cancels it, and a cross-origin script URL fires a simple `error`;
//! 4. `terminate()` drops the worker's queued messages and stops the thread, including a script spinning in a
//!    loop (at its next call into a worker native);
//! 5. a MessageChannel port pair across page and worker, both directions, a port shipped while a message to
//!    it is in flight, and `e.ports` on the page's MessageEvent;
//! 6. an idle callback's IdleDeadline (timeRemaining within the frame budget, didTimeout), cancelIdleCallback,
//!    and a callback whose timeout passed (didTimeout, timeRemaining 0);
//! 7. a same-origin http(s) worker fetched through the page's loader, its `importScripts`, and a module
//!    worker's static import.
//!
//! No WPT: the seat runs the suites. Storage lives under CARGO_TARGET_TMPDIR.

use std::io::{Read, Write};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("aether-workers-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        aether::storage::set_root_dir(Some(dir));
    });
}

/// A `data:` URL carrying `src` (every byte but [A-Za-z0-9] percent-encoded, so it nests in a JS string).
fn data_url(src: &str) -> String {
    let mut out = String::from("data:text/javascript,");
    for b in src.bytes() {
        if b.is_ascii_alphanumeric() {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn page(url: &str, script: &str) -> aether::AetherEngine {
    init();
    let mut engine = aether::AetherEngine::new();
    let html = format!("<!doctype html><html><body><script>{script}</script></body></html>");
    engine.load_html_styled(url, &html, &[], true);
    engine
}

/// Drives the page's event loop until `cond` (JS) is truthy; answers `out` (JS).
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

/// Drives the event loop for `ms` of real time.
fn spin(engine: &mut aether::AetherEngine, ms: u64) {
    let js = engine.js_engine.as_mut().expect("engine");
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        js.tick();
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn eval(engine: &mut aether::AetherEngine, src: &str) -> String {
    engine.js_engine.as_mut().expect("engine").eval_string(src).unwrap_or_else(|e| format!("<error {e}>"))
}

const PAGE: &str = "http://127.0.0.1:9/page";

// ----------------------------------------------------------------------------------------------------- 1

#[test]
fn a_worker_echoes_a_message() {
    let w = data_url("onmessage = function (e) { postMessage({ echo: e.data.n * 2, name: self.name, scope: String(self), proto: self instanceof EventTarget }); };");
    let mut p = page(
        PAGE,
        &format!(
            "var out = null; var w = new Worker('{w}', {{ name: 'echo' }});
             w.onmessage = function (e) {{ out = [e.data.echo, e.data.name, e.data.scope, e.data.proto, e.origin === '', e.ports.length, e.isTrusted].join('|'); }};
             w.postMessage({{ n: 21 }});"
        ),
    );
    assert_eq!(run_until(&mut p, "out !== null", "out", 20), "42|echo|[object DedicatedWorkerGlobalScope]|true|true|0|true");
}

// ----------------------------------------------------------------------------------------------------- 2

#[test]
fn a_transferred_array_buffer_is_detached_on_the_sender() {
    let w = data_url(
        "onmessage = function (e) {
            var back = new Uint8Array([5, 6]).buffer;
            postMessage({ len: e.data.byteLength, b0: new Uint8Array(e.data)[0], back: back }, [back]);
            postMessage({ backDetached: back.byteLength === 0 && back.detached === true });
        };",
    );
    let mut p = page(
        PAGE,
        &format!(
            "var got = []; var w = new Worker('{w}');
             w.onmessage = function (e) {{ got.push(e.data); }};
             var buf = new Uint8Array([7, 8, 9]).buffer;
             w.postMessage(buf, [buf]);
             var after = buf.byteLength + ':' + buf.detached;
             var dup = 'none'; try {{ w.postMessage(buf, [buf]); }} catch (x) {{ dup = x.name; }}"
        ),
    );
    assert_eq!(eval(&mut p, "after"), "0:true", "the sender's buffer is detached at once");
    assert_eq!(eval(&mut p, "dup"), "DataCloneError", "a detached buffer cannot be transferred again");
    assert_eq!(
        run_until(&mut p, "got.length === 2", "[got[0].len, got[0].b0, got[0].back.byteLength, new Uint8Array(got[0].back)[1], got[1].backDetached].join('|')", 20),
        "3|7|2|6|true"
    );
}

// ----------------------------------------------------------------------------------------------------- 3

#[test]
fn an_uncaught_worker_error_fires_error_at_the_worker() {
    let thrower = data_url("setTimeout(function () { throw new TypeError('boom'); }, 0);");
    let syntax = data_url("var ok = 1;\nvar = ;\n");
    let handled = data_url("self.onerror = function (m) { postMessage('seen:' + m); return true; }; throw new Error('x');");
    let mut p = page(
        PAGE,
        &format!(
            "var a = null, b = null, c = null, cerr = 0, d = null;
             var w1 = new Worker('{thrower}');
             w1.onerror = function (e) {{ a = [e.constructor.name, e.message, e.filename.indexOf('data:') === 0, e.lineno].join('|'); }};
             var w2 = new Worker('{syntax}');
             w2.addEventListener('error', function (e) {{ b = e.message.split(':')[0] + '|' + e.lineno; }});
             var w3 = new Worker('{handled}');
             w3.onmessage = function (e) {{ c = e.data; }};
             w3.onerror = function () {{ cerr++; }};
             var w4 = new Worker('http://elsewhere.invalid/w.js');
             w4.onerror = function (e) {{ d = e.constructor.name + '|' + e.type; }};"
        ),
    );
    assert_eq!(run_until(&mut p, "a !== null", "a", 20), "ErrorEvent|Uncaught TypeError: boom|true|0");
    assert_eq!(run_until(&mut p, "b !== null", "b", 20), "Uncaught SyntaxError|2", "a SyntaxError's line is the worker script's");
    assert_eq!(run_until(&mut p, "c !== null", "c", 20), "seen:Uncaught Error: x");
    assert_eq!(run_until(&mut p, "d !== null", "d", 20), "Event|error", "a cross-origin worker script is a fetch failure: a simple error event");
    spin(&mut p, 100);
    assert_eq!(eval(&mut p, "cerr"), "0", "the worker's onerror returning true canceled the page's error event");
}

// ----------------------------------------------------------------------------------------------------- 4

#[test]
fn terminate_stops_the_worker() {
    let ticker = data_url("var n = 0; setInterval(function () { postMessage(++n); }, 5);");
    let spinner = data_url("onmessage = function () { postMessage('spinning'); for (;;) { performance.now(); } };");
    let mut p = page(
        PAGE,
        &format!(
            "var got = 0; var w = new Worker('{ticker}'); w.onmessage = function (e) {{ got = e.data; }};
             var s = null; var w2 = new Worker('{spinner}'); w2.onmessage = function (e) {{ s = e.data; }}; w2.postMessage(0);"
        ),
    );
    let doc = aether::js::broadcast::doc();
    run_until(&mut p, "got >= 2", "got", 20);
    assert_eq!(run_until(&mut p, "s !== null", "s", 20), "spinning");
    assert_eq!(aether::workers::dedicated::live(doc), 2);
    assert_eq!(aether::workers::dedicated::threads(doc), 2);
    eval(&mut p, "w.terminate(); w2.terminate(); var at = got; w.postMessage('after terminate');");
    assert_eq!(aether::workers::dedicated::live(doc), 0, "terminate() unregisters at once");
    spin(&mut p, 150);
    assert_eq!(eval(&mut p, "got === at"), "true", "no message of a terminated worker reaches the page");
    // The spinning worker's thread stops at its next call into a worker native (performance.now()).
    let end = Instant::now() + Duration::from_secs(10);
    while aether::workers::dedicated::threads(doc) > 0 {
        assert!(Instant::now() < end, "the worker threads did not stop");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ----------------------------------------------------------------------------------------------------- 5

#[test]
fn a_port_pair_across_page_and_worker() {
    let w = data_url(
        "onmessage = function (e) {
            if (e.data === 'give') {
                var c2 = new MessageChannel();
                postMessage('here', [c2.port1]);
                c2.port2.postMessage('via-port');
                return;
            }
            var p = e.data.port;
            var same = e.ports[0] === p;
            p.onmessage = function (m) { p.postMessage(m.data + '-pong|' + same + '|' + (p instanceof MessagePort)); };
        };",
    );
    let mut p = page(
        PAGE,
        &format!(
            "var out = null, out2 = null, ports = -1; var w = new Worker('{w}');
             var ch = new MessageChannel();
             var kinds = [ch.port1 instanceof MessagePort, String(ch.port2)].join('|');
             w.postMessage({{ port: ch.port2 }}, [ch.port2]);
             ch.port1.onmessage = function (e) {{ out = e.data; }};
             ch.port1.postMessage('ping');
             var shipped = 'ok'; try {{ ch.port2.postMessage('dropped'); }} catch (x) {{ shipped = x.name; }}
             var clone = 'none'; try {{ w.postMessage(ch.port1); }} catch (x) {{ clone = x.name; }}
             w.onmessage = function (e) {{
                 if (e.data !== 'here') {{ return; }}
                 ports = e.ports.length;
                 e.ports[0].onmessage = function (m) {{ out2 = m.data; }};
             }};
             w.postMessage('give');"
        ),
    );
    assert_eq!(eval(&mut p, "kinds"), "true|[object MessagePort]");
    assert_eq!(eval(&mut p, "shipped"), "ok", "posting on a shipped port is a no-op");
    assert_eq!(eval(&mut p, "clone"), "DataCloneError", "a port outside the transfer list cannot be cloned");
    assert_eq!(run_until(&mut p, "out !== null", "out", 20), "ping-pong|true|true", "the message posted while the port was in flight arrives");
    assert_eq!(run_until(&mut p, "out2 !== null", "out2 + '|' + ports", 20), "via-port|1");
}

// ----------------------------------------------------------------------------------------------------- 6

#[test]
fn an_idle_callback_gets_a_deadline() {
    let mut p = page(
        PAGE,
        "var r = null, cancelled = 'no';
         requestIdleCallback(function (d) {
             var t = d.timeRemaining();
             r = [typeof t, t >= 0 && t <= 16, d.didTimeout, String(d), d instanceof IdleDeadline].join('|');
         });
         var c = requestIdleCallback(function () { cancelled = 'ran'; });
         cancelIdleCallback(c);
         var bad = 'none'; try { requestIdleCallback(5); } catch (x) { bad = x.name; }",
    );
    assert_eq!(eval(&mut p, "bad"), "TypeError");
    assert_eq!(run_until(&mut p, "r !== null", "r", 20), "number|true|false|[object IdleDeadline]|true");
    assert_eq!(eval(&mut p, "cancelled"), "no", "a cancelled idle callback never runs");
    // A callback whose timeout has passed by the next pass runs as timed out: didTimeout, no time remaining.
    eval(
        &mut p,
        "var t = null;
         requestIdleCallback(function (d) { t = d.didTimeout + '|' + d.timeRemaining(); }, { timeout: 1 });
         var s0 = performance.now(); while (performance.now() - s0 < 5) {}",
    );
    assert_eq!(run_until(&mut p, "t !== null", "t", 20), "true|0");
}

// ----------------------------------------------------------------------------------------------------- 7

/// A blocking fixture server: `route(path)` → (content type, body); 404 otherwise.
fn server(route: fn(&str) -> Option<(&'static str, String)>) -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for c in l.incoming() {
            let Ok(mut c) = c else { continue };
            std::thread::spawn(move || {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    match c.read(&mut tmp) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head.split(' ').nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_string();
                let (status, ctype, body) = match route(&path) {
                    Some((t, b)) => ("200 OK", t, b),
                    None => ("404 Not Found", "text/plain", String::from("no")),
                };
                let _ = write!(c, "HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{body}", body.len());
                let _ = c.flush();
            });
        }
    });
    port
}

#[test]
fn a_same_origin_worker_loads_through_the_page_loader() {
    let port = server(|path| match path {
        "/w.js" => Some(("text/javascript", "importScripts('lib.js'); onmessage = function (e) { postMessage(lib(e.data) + '|' + location.pathname); };".into())),
        "/lib.js" => Some(("text/javascript", "function lib(x) { return x + '!'; }".into())),
        "/m.js" => Some(("text/javascript", "import { f } from './dep.js'; onmessage = function (e) { var i = 'none'; try { importScripts('lib.js'); } catch (x) { i = x.name; } postMessage(f(e.data) + '|' + i); };".into())),
        "/dep.js" => Some(("text/javascript", "export function f(x) { return 'mod:' + x; }".into())),
        _ => None,
    });
    let origin = format!("http://127.0.0.1:{port}");
    let mut p = page(
        &format!("{origin}/page"),
        "var a = null, b = null;
         var w = new Worker('/w.js'); w.onmessage = function (e) { a = e.data; }; w.postMessage('hi');
         var m = new Worker('/m.js', { type: 'module' }); m.onmessage = function (e) { b = e.data; }; m.postMessage('x');",
    );
    assert_eq!(run_until(&mut p, "a !== null", "a", 20), "hi!|/w.js");
    assert_eq!(run_until(&mut p, "b !== null", "b", 20), "mod:x|TypeError", "a module worker imports statically and refuses importScripts");
}
