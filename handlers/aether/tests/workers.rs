//! AETHERWORKERS (LEDGER SR123): dedicated workers, MessageChannel/MessagePort and requestIdleCallback, one
//! host test per rule, each on its own page (its own document, its own inbox):
//!
//! 1. a worker echoes a message (DedicatedWorkerGlobalScope, `name`, MessageEvent origin "" and no ports);
//! 2. an ArrayBuffer in the transfer list is detached on the sender, both ways;
//! 3. an uncaught worker error fires an ErrorEvent at the Worker (message, filename, the throw site's lineno — JSPOS),
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
//! AETHERWORKERS2 (LEDGER SR125), one test per item:
//!
//! 8. `terminate()` interrupts a loop that calls nothing (js_core `Vm::interrupt`);
//! 9. the worker script goes out as `Sec-Fetch-Dest: worker` (importScripts as `script`) and a non-JavaScript
//!    MIME type is refused, the console naming it;
//! 10. the Worker's ErrorEvent is cancelable (uncanceled, it reaches the Window) and `new MessageEvent(type,
//!     { ports })` carries its ports;
//! 11. IndexedDB, Web Locks and BroadcastChannel in a worker share the owner document's storage;
//! 12. `unhandledrejection` / `rejectionhandled` at the worker global;
//! 13. nested workers under the parent's captured URL, origin and CSP, terminated with it.
//!
//! AETHERWORKERS3 (LEDGER SR129):
//!
//! 14. a SharedWorker outlives the document that started it, serves a second document's `connect`, and ends
//!     when its last client port closes;
//! 15. the worker script is fetched on the worker's thread (`new Worker()` returns before a slow script
//!     arrives) and the worker's CSP is its script response's (`eval`, `fetch`, `importScripts`), not the
//!     document's;
//! 16. a `data:` worker has an opaque origin: IndexedDB and the Cache API refuse with SecurityError;
//! 17. ImageBitmap (serializable and transferable) and OffscreenCanvas (transferable) through structuredClone
//!     and a MessagePort, the detach rules, a ReadableStream refused by name, `messageerror` in a worker realm;
//! 18. port garbage collection in a worker realm: an unreachable local pair is closed by the realm's collector,
//!     a port whose peer is in the page is kept until that peer closes;
//! 19. the AETHERSW (SR104) client gaps: `navigator.serviceWorker.ready` is one promise, `clients.matchAll`
//!     filters by `type` (an invalid one rejects), a WindowClient's `visibilityState` / `focused` and its
//!     `focus()` rejecting InvalidAccessError without user activation.
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
    assert_eq!(run_until(&mut p, "a !== null", "a", 20), "ErrorEvent|Uncaught TypeError: boom|true|1");
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

// ------------------------------------------------------------------------- AETHERWORKERS2 (SR125) item 1

/// `terminate()` of a worker spinning in a loop that calls nothing: the kill flag is the realm's
/// `Vm::interrupt`, which js_core checks beside the instruction budget, so the thread stops at its next
/// instruction. Without it the loop runs out its whole task budget (`js::TASK_BUDGET`, 400M instructions,
/// seconds of interpretation); the bound below is far inside one budget.
#[test]
fn terminate_interrupts_a_loop_that_calls_nothing() {
    let spinner = data_url("postMessage('spinning'); while (true) {}");
    let mut p = page(PAGE, &format!("var s = null; var w = new Worker('{spinner}'); w.onmessage = function (e) {{ s = e.data; }};"));
    let doc = aether::js::broadcast::doc();
    assert_eq!(run_until(&mut p, "s !== null", "s", 20), "spinning");
    assert_eq!(aether::workers::dedicated::threads(doc), 1, "the worker thread is spinning");
    eval(&mut p, "w.terminate();");
    let t0 = Instant::now();
    while aether::workers::dedicated::threads(doc) > 0 {
        assert!(t0.elapsed() < Duration::from_millis(1000), "the spinning worker was not interrupted within one TASK_BUDGET");
        std::thread::sleep(Duration::from_millis(5));
    }
}

// ------------------------------------------------------------------------- AETHERWORKERS2 (SR125) item 2

/// (path, `Sec-Fetch-Dest`) of every request [`recording_server`] answered.
static SEEN: std::sync::Mutex<Vec<(String, String)>> = std::sync::Mutex::new(Vec::new());

/// [`server`] that also records each request's path and `Sec-Fetch-Dest` header in [`SEEN`].
fn recording_server(route: fn(&str) -> Option<(&'static str, String)>) -> u16 {
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
                let dest = head
                    .lines()
                    .find_map(|l| l.split_once(':').filter(|(k, _)| k.trim().eq_ignore_ascii_case("sec-fetch-dest")).map(|(_, v)| v.trim().to_string()))
                    .unwrap_or_default();
                SEEN.lock().unwrap().push((path.clone(), dest));
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

fn seen_dest(port_path: &str) -> String {
    SEEN.lock().unwrap().iter().rev().find(|(p, _)| p == port_path).map(|(_, d)| d.clone()).unwrap_or_else(|| "<not requested>".into())
}

/// The worker script is requested with destination `worker` (`Sec-Fetch-Dest: worker`), its `importScripts`
/// with destination `script`; a script served as `text/html` is refused (HTML §10.2.4: not a JavaScript MIME
/// type) — a simple `error` event at the Worker, and the console names the MIME type.
#[test]
fn a_worker_script_is_fetched_as_a_worker_and_must_be_javascript() {
    let port = recording_server(|path| match path {
        "/mime/ok.js" => Some(("text/javascript; charset=utf-8", "importScripts('lib2.js'); postMessage(lib2());".into())),
        "/mime/lib2.js" => Some(("application/javascript", "function lib2() { return 'ran'; }".into())),
        "/mime/html.js" => Some(("text/html", "postMessage('should not run');".into())),
        _ => None,
    });
    let origin = format!("http://127.0.0.1:{port}");
    let mut p = page(
        &format!("{origin}/mime/page"),
        "var a = null, b = null;
         var ok = new Worker('/mime/ok.js'); ok.onmessage = function (e) { a = e.data; };
         var bad = new Worker('/mime/html.js');
         bad.onmessage = function () { b = 'ran'; };
         bad.onerror = function (e) { b = e.constructor.name + '|' + e.type; };",
    );
    assert_eq!(run_until(&mut p, "a !== null", "a", 20), "ran");
    assert_eq!(run_until(&mut p, "b !== null", "b", 20), "Event|error", "a text/html worker script is a fetch failure");
    assert_eq!(seen_dest("/mime/ok.js"), "worker", "Sec-Fetch-Dest of the worker script");
    assert_eq!(seen_dest("/mime/lib2.js"), "script", "Sec-Fetch-Dest of importScripts");
    let console = aether::js::console_lines();
    assert!(console.iter().any(|(_, l)| l.contains("MIME type ('text/html')")), "the console names the refused MIME type: {console:?}");
}

// ------------------------------------------------------------------------- AETHERWORKERS2 (SR125) item 6

/// A worker creates workers: the child's URL resolves against the parent worker's location, its messages
/// arrive at the parent's `Worker` object, a cross-origin child is a simple `error` at the parent, a `data:`
/// child is refused by the parent's CSP (`worker-src 'self'`), and terminating the parent terminates the child.
///
/// AETHERWORKERS3 (SR129): the parent's policy is its OWN — the `Content-Security-Policy` header of its script
/// response (HTML §10.2.4 step 13.3), which the fixture now sends; the document's `<meta>` policy governs only
/// the parent's own fetch. (Under AETHERWORKERS2 the parent captured the document's policy instead.)
#[test]
fn a_worker_creates_nested_workers_under_its_captured_context() {
    let port = server_ex(|path| match path {
        "/w/parent.js" => Some(Fixture::js(
            "var c = new Worker('child.js', { name: 'kid' });
             c.onmessage = function (e) { postMessage('child:' + e.data); };
             var x = new Worker('http://elsewhere.invalid/x.js');
             x.onerror = function (e) { postMessage('xo:' + e.constructor.name); };
             var d = new Worker('data:text/javascript,postMessage(1)');
             d.onmessage = function () { postMessage('csp:ran'); };
             d.onerror = function (e) { postMessage('csp:' + e.type); };",
        )
        .header("Content-Security-Policy", "worker-src 'self'")),
        "/w/child.js" => Some(Fixture::js("postMessage(location.pathname + '|' + name); setInterval(function () {}, 5);")),
        _ => None,
    });
    init();
    let mut p = aether::AetherEngine::new();
    let html = "<!doctype html><html><head><meta http-equiv=\"Content-Security-Policy\" content=\"worker-src 'self'\"></head><body><script>
        var got = []; var w = new Worker('/w/parent.js'); w.onmessage = function (e) { got.push(e.data); };
        </script></body></html>";
    p.load_html_styled(&format!("http://127.0.0.1:{port}/page"), html, &[], true);
    let doc = aether::js::broadcast::doc();
    assert_eq!(run_until(&mut p, "got.length >= 3", "got.slice().sort().join(' ; ')", 20), "child:/w/child.js|kid ; csp:error ; xo:Event");
    assert_eq!(aether::workers::dedicated::threads(doc), 2, "the parent and its child run");
    eval(&mut p, "w.terminate();");
    let end = Instant::now() + Duration::from_secs(5);
    while aether::workers::dedicated::threads(doc) > 0 {
        assert!(Instant::now() < end, "terminating the parent did not stop its child");
        std::thread::sleep(Duration::from_millis(10));
    }
}

// ------------------------------------------------------------------------- AETHERWORKERS2 (SR125) item 3

/// The ErrorEvent a worker error fires at the Worker is cancelable (HTML §10.2.5): canceled there, the
/// Window never hears it; not canceled, it is reported again at the Window as a cancelable ErrorEvent. And
/// `new MessageEvent(type, { ports })` from script carries its ports (a frozen array; a non-port throws).
#[test]
fn the_worker_error_event_is_cancelable_and_message_event_init_has_ports() {
    let up = data_url("throw new Error('up');");
    let kept = data_url("throw new Error('kept');");
    let mut p = page(
        PAGE,
        &format!(
            "var a = null, b = null, winA = 0, winB = null;
             window.addEventListener('error', function (e) {{
                 if (String(e.message).indexOf('up') >= 0) {{ winA++; }}
                 if (String(e.message).indexOf('kept') >= 0) {{ winB = e.constructor.name + '|' + e.cancelable; e.preventDefault(); }}
             }});
             var w1 = new Worker('{up}'); w1.onerror = function (e) {{ a = e.cancelable + '|' + e.message; e.preventDefault(); }};
             var w2 = new Worker('{kept}'); w2.onerror = function (e) {{ b = String(e.cancelable); }};
             var ch = new MessageChannel();
             var me = new MessageEvent('message', {{ data: 1, ports: [ch.port1] }});
             var mp = [me.ports.length, me.ports[0] === ch.port1, Object.isFrozen(me.ports), new MessageEvent('x').ports.length, me.ports === me.ports].join('|');
             var bad = 'none'; try {{ new MessageEvent('x', {{ ports: [{{}}] }}); }} catch (x) {{ bad = x.name; }}"
        ),
    );
    assert_eq!(eval(&mut p, "mp"), "1|true|true|0|true", "MessageEventInit.ports");
    assert_eq!(eval(&mut p, "bad"), "TypeError", "a non-MessagePort in ports");
    assert_eq!(run_until(&mut p, "a !== null", "a", 20), "true|Uncaught Error: up");
    assert_eq!(run_until(&mut p, "winB !== null", "winB + '|' + b", 20), "ErrorEvent|true|true", "not canceled at the Worker, the error reaches the Window");
    spin(&mut p, 100);
    assert_eq!(eval(&mut p, "winA"), "0", "canceled at the Worker, the Window never hears it");
}

// ------------------------------------------------------------------------- AETHERWORKERS2 (SR125) item 4

/// IndexedDB, Web Locks and BroadcastChannel inside a worker run over the owner document's storage key: a
/// record the worker writes is the document's to read, a lock the document holds queues the worker's request
/// until it is released (the grant arrives at the worker's own inbox), and a BroadcastChannel of the same
/// name carries messages both ways between the document and the worker.
#[test]
fn a_worker_shares_its_documents_storage() {
    let port = server(|path| match path {
        "/s/w.js" => Some((
            "text/javascript",
            "var bc = new BroadcastChannel('wk');
             bc.onmessage = function (e) { postMessage('bc:' + e.data); };
             onmessage = function (e) {
                 if (e.data !== 'go') { return; }
                 navigator.locks.request('L', function (lock) { postMessage('lock:' + lock.name + ':' + lock.mode); });
                 navigator.locks.request('L2', function () { postMessage('lock2'); });
                 var r = indexedDB.open('wdb', 1);
                 r.onupgradeneeded = function () { r.result.createObjectStore('s'); };
                 r.onsuccess = function () {
                     var db = r.result;
                     var tx = db.transaction('s', 'readwrite');
                     tx.objectStore('s').put('from-worker', 'k');
                     tx.oncomplete = function () { db.close(); bc.postMessage('from-worker'); postMessage('idb:done'); };
                 };
             };"
                .into(),
        )),
        _ => None,
    });
    let mut p = page(
        &format!("http://127.0.0.1:{port}/s/page"),
        "var got = []; var release = null;
         navigator.locks.request('L2', function () { return new Promise(function (res) { release = res; }); });
         var pbc = new BroadcastChannel('wk');
         pbc.onmessage = function (e) { got.push('page-bc:' + e.data); };
         var w = new Worker('/s/w.js'); w.onmessage = function (e) { got.push(e.data); };
         w.postMessage('go');",
    );
    run_until(&mut p, "got.indexOf('idb:done') >= 0 && got.indexOf('lock:L:exclusive') >= 0 && got.indexOf('page-bc:from-worker') >= 0", "got.join(',')", 20);
    spin(&mut p, 100);
    assert_eq!(eval(&mut p, "got.indexOf('lock2')"), "-1", "the document holds L2: the worker's request waits");
    eval(&mut p, "release(); pbc.postMessage('hello');");
    run_until(&mut p, "got.indexOf('lock2') >= 0 && got.indexOf('bc:hello') >= 0", "got.join(',')", 20);
    eval(
        &mut p,
        "var v = null; var r2 = indexedDB.open('wdb', 1);
         r2.onsuccess = function () { var q = r2.result.transaction('s').objectStore('s').get('k'); q.onsuccess = function () { v = q.result; }; };",
    );
    assert_eq!(run_until(&mut p, "v !== null", "v", 20), "from-worker", "the worker's record is in the document's database");
}

// ------------------------------------------------------------------------- AETHERWORKERS2 (SR125) item 5

/// Rejection tracking in the worker realm: an unhandled rejection fires a cancelable PromiseRejectionEvent
/// `unhandledrejection` at the worker global, and a handler attached later fires `rejectionhandled`.
#[test]
fn a_worker_fires_unhandledrejection_and_rejectionhandled() {
    let w = data_url(
        "self.addEventListener('unhandledrejection', function (e) {
             postMessage('un:' + e.reason + ':' + (e.promise instanceof Promise) + ':' + e.cancelable + ':' + (e instanceof PromiseRejectionEvent));
             e.preventDefault();
             var p = e.promise;
             setTimeout(function () { p.catch(function () {}); }, 0);
         });
         self.onrejectionhandled = function (e) { postMessage('handled:' + e.reason); };
         Promise.reject('r1');",
    );
    let mut p = page(PAGE, &format!("var got = []; var w = new Worker('{w}'); w.onmessage = function (e) {{ got.push(e.data); }};"));
    assert_eq!(run_until(&mut p, "got.length >= 2", "got.join(',')", 20), "un:r1:true:true:true,handled:r1");
}

// ------------------------------------------------------------------------------ AETHERWORKERS3 (SR129) fixtures

/// A fixture response: content type, extra headers, body, and a delay before the answer.
struct Fixture {
    ctype: &'static str,
    headers: Vec<(&'static str, String)>,
    body: String,
    delay_ms: u64,
}

impl Fixture {
    fn js(body: &str) -> Fixture {
        Fixture { ctype: "text/javascript", headers: Vec::new(), body: body.to_string(), delay_ms: 0 }
    }
    fn text(body: &str) -> Fixture {
        Fixture { ctype: "text/plain", headers: Vec::new(), body: body.to_string(), delay_ms: 0 }
    }
    fn header(mut self, k: &'static str, v: &str) -> Fixture {
        self.headers.push((k, v.to_string()));
        self
    }
    fn delayed(mut self, ms: u64) -> Fixture {
        self.delay_ms = ms;
        self
    }
}

/// [`server`] whose routes answer a [`Fixture`] (extra headers, a delay); 404 otherwise.
fn server_ex(route: fn(&str) -> Option<Fixture>) -> u16 {
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
                let (status, f) = match route(&path) {
                    Some(f) => ("200 OK", f),
                    None => ("404 Not Found", Fixture::text("no")),
                };
                if f.delay_ms > 0 {
                    std::thread::sleep(Duration::from_millis(f.delay_ms));
                }
                let extra: String = f.headers.iter().map(|(k, v)| format!("{k}: {v}\r\n")).collect();
                let _ = write!(
                    c,
                    "HTTP/1.1 {status}\r\nContent-Type: {}\r\n{extra}Content-Length: {}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n{}",
                    f.ctype,
                    f.body.len(),
                    f.body
                );
                let _ = c.flush();
            });
        }
    });
    port
}

// ------------------------------------------------------------------------- AETHERWORKERS3 (SR129) item 1

/// HTML §10.2.6.4 / §10.2.3: a SharedWorker started by document A serves document B's `connect` (one realm:
/// its connection counter reaches 2; `name`, a SharedWorkerGlobalScope, `e.source` the connect port), keeps
/// running after A's thread ends and A retires, and is terminated when B — its last client — closes its port.
#[test]
fn a_shared_worker_outlives_its_first_document_and_ends_with_its_last_port() {
    let w = data_url(
        "var n = 0;
         onconnect = function (e) {
             n++;
             var p = e.ports[0];
             var src = e.source === p;
             p.onmessage = function (m) { p.postMessage([m.data, n, name, String(self), src, typeof postMessage].join(':')); };
         };",
    );
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
    let wa = w.clone();
    let a = std::thread::spawn(move || {
        let mut p = page(PAGE, &format!("var a = null; var sw = new SharedWorker('{wa}', 'outlive'); sw.port.onmessage = function (e) {{ a = e.data; }}; sw.port.postMessage('A');"));
        let got = run_until(&mut p, "a !== null", "a", 20);
        tx.send(got).unwrap();
        // Document A stays alive until B has connected; then this thread ends and A retires.
        let _ = go_rx.recv_timeout(Duration::from_secs(30));
    });
    assert_eq!(rx.recv_timeout(Duration::from_secs(30)).unwrap(), "A:1:outlive:[object SharedWorkerGlobalScope]:true:undefined");
    let mut b = page(PAGE, &format!("var b = []; var sw = new SharedWorker('{w}', {{ name: 'outlive' }}); sw.port.onmessage = function (e) {{ b.push(e.data); }}; sw.port.postMessage('B1');"));
    assert_eq!(run_until(&mut b, "b.length >= 1", "b[0]", 20), "B1:2:outlive:[object SharedWorkerGlobalScope]:true:undefined", "B connects to A's worker");
    go_tx.send(()).unwrap();
    a.join().unwrap();
    eval(&mut b, "sw.port.postMessage('B2');");
    assert_eq!(run_until(&mut b, "b.length >= 2", "b[1]", 20), "B2:2:outlive:[object SharedWorkerGlobalScope]:true:undefined", "the worker outlived document A");
    assert!(aether::workers::shared::is_running(&w, "outlive"), "still registered while B is a client");
    eval(&mut b, "sw.port.close();");
    let end = Instant::now() + Duration::from_secs(5);
    while aether::workers::shared::is_running(&w, "outlive") {
        assert!(Instant::now() < end, "the shared worker did not end with its last client port");
        std::thread::sleep(Duration::from_millis(10));
    }
    // A different name is a different worker; a type mismatch for a running one is an error at the object.
    let mut c = page(
        PAGE,
        &format!(
            "var r = null; var k1 = new SharedWorker('{w}', 'kind'); k1.port.start();
             var k2 = new SharedWorker('{w}', {{ name: 'kind', type: 'module' }});
             k2.onerror = function (e) {{ r = e.type; }};"
        ),
    );
    assert_eq!(run_until(&mut c, "r !== null", "r", 20), "error");
}

// ------------------------------------------------------------------------- AETHERWORKERS3 (SR129) item 2

/// The worker script is fetched on the worker's own thread: `new Worker()` returns at once although the script
/// answers after 1.5 s. The worker's policy container is its script response's (HTML §10.2.4 step 13.3):
/// a worker served with `script-src 'self'; connect-src 'none'` cannot `eval`, `fetch` or `importScripts` a
/// `data:` URL, while a worker served with no policy, in a document whose `<meta>` policy forbids `eval`
/// and every connection, can do both.
#[test]
fn a_worker_fetches_its_script_on_its_own_thread_and_takes_its_own_csp() {
    let port = server_ex(|path| match path {
        "/csp/strict.js" => Some(
            Fixture::js(
                "var out = [];
                 try { eval('1'); out.push('eval:ok'); } catch (e) { out.push('eval:' + e.name); }
                 try { importScripts('lib.js'); out.push('import:' + lib()); } catch (e) { out.push('import:' + e.name); }
                 try { importScripts('data:text/javascript,1'); out.push('import-data:ok'); } catch (e) { out.push('import-data:' + e.name); }
                 fetch('data.txt').then(function (r) { out.push('fetch:' + r.status); postMessage(out.join(',')); },
                                        function (e) { out.push('fetch:' + e.name); postMessage(out.join(',')); });",
            )
            .header("Content-Security-Policy", "script-src 'self'; connect-src 'none'"),
        ),
        "/csp/loose.js" => Some(Fixture::js(
            "var out = [];
             try { out.push('eval:' + eval('6 * 7')); } catch (e) { out.push('eval:' + e.name); }
             fetch('data.txt').then(function (r) { out.push('fetch:' + r.status); postMessage(out.join(',')); },
                                    function (e) { out.push('fetch:' + e.name); postMessage(out.join(',')); });",
        )),
        "/csp/slow.js" => Some(Fixture::js("postMessage('slow');").delayed(1500)),
        "/csp/lib.js" => Some(Fixture::js("function lib() { return 'ok'; }")),
        "/csp/data.txt" => Some(Fixture::text("data")),
        _ => None,
    });
    init();
    let mut p = aether::AetherEngine::new();
    let html = "<!doctype html><html><head><meta http-equiv=\"Content-Security-Policy\" content=\"worker-src 'self'; connect-src 'none'; script-src 'self' 'unsafe-inline'\"></head><body><script>
        var strict = null, loose = null, slow = null;
        var t0 = Date.now();
        var w0 = new Worker('/csp/slow.js'); w0.onmessage = function (e) { slow = e.data; };
        var dt = Date.now() - t0;
        var w1 = new Worker('/csp/strict.js'); w1.onmessage = function (e) { strict = e.data; };
        var w2 = new Worker('/csp/loose.js'); w2.onmessage = function (e) { loose = e.data; };
        </script></body></html>";
    p.load_html_styled(&format!("http://127.0.0.1:{port}/csp/page"), html, &[], true);
    let dt: f64 = eval(&mut p, "dt").parse().unwrap_or(f64::MAX);
    assert!(dt < 750.0, "new Worker() waited {dt} ms for the script: the fetch is not on the worker's thread");
    assert_eq!(run_until(&mut p, "strict !== null", "strict", 20), "eval:EvalError,import:ok,import-data:NetworkError,fetch:TypeError", "the worker's own policy");
    assert_eq!(run_until(&mut p, "loose !== null", "loose", 20), "eval:42,fetch:200", "the document's policy does not bind an http(s) worker");
    assert_eq!(run_until(&mut p, "slow !== null", "slow", 20), "slow");
}

// ------------------------------------------------------------------------- AETHERWORKERS3 (SR129) item 2b

/// A `data:` worker's origin is opaque (HTML §10.2.4: "if worker URL's scheme is data, then … a new opaque
/// origin"), so it has no storage key: IndexedDB and the Cache API refuse with SecurityError, where a
/// same-origin worker shares its document's (`a_worker_shares_its_documents_storage`).
#[test]
fn a_data_worker_has_an_opaque_origin_and_no_storage() {
    let w = data_url(
        "var out = [];
         var fin = function (x) { out.push(x); if (out.length === 2) { postMessage(out.sort().join('|')); } };
         try {
             var r = indexedDB.open('d');
             r.onsuccess = function () { fin('idb:ok'); };
             r.onerror = function () { fin('idb:' + (r.error && r.error.name)); };
         } catch (e) { fin('idb:' + e.name); }
         try {
             (typeof caches === 'undefined' ? Promise.reject({ name: 'undefined' }) : caches.open('c'))
                 .then(function () { fin('caches:ok'); }, function (e) { fin('caches:' + e.name); });
         } catch (e) { fin('caches:' + e.name); }",
    );
    let mut p = page(PAGE, &format!("var got = null; var w = new Worker('{w}'); w.onmessage = function (e) {{ got = e.data; }};"));
    assert_eq!(run_until(&mut p, "got !== null", "got", 20), "caches:SecurityError|idb:SecurityError");
}

// ------------------------------------------------------------------------- AETHERWORKERS3 (SR129) item 4

/// HTML §2.7 / §4.12.5.3: an ImageBitmap is serializable (a copy keeps its pixels) and transferable (the
/// source is closed; a closed one cannot be transferred or cloned again); an OffscreenCanvas is transferable
/// only (the receiver gets its size, the source is detached and refuses `getContext`; one with a context
/// throws InvalidStateError; cloning one without transfer throws DataCloneError); a ReadableStream in the
/// transfer list is refused by name. Across a page MessageChannel the bitmap arrives whole; posted to a worker
/// (whose realm has no canvas objects) it fires `messageerror` there.
#[test]
fn image_bitmaps_and_offscreen_canvases_transfer() {
    let w = data_url("onmessage = function () { postMessage('message'); }; onmessageerror = function () { postMessage('messageerror'); };");
    let mut p = page(
        PAGE,
        &format!(
            "var oc = new OffscreenCanvas(2, 1); var cx = oc.getContext('2d');
             cx.fillStyle = 'rgb(255, 0, 0)'; cx.fillRect(0, 0, 1, 1); cx.fillStyle = 'rgb(0, 0, 255)'; cx.fillRect(1, 0, 1, 1);
             var bmp = oc.transferToImageBitmap();
             var px = function (b) {{ var c = new OffscreenCanvas(b.width, b.height).getContext('2d'); c.drawImage(b, 0, 0); return Array.from(c.getImageData(0, 0, b.width, b.height).data).join(','); }};
             var copy = structuredClone(bmp);
             var r1 = [copy instanceof ImageBitmap, copy.width, copy.height, px(copy), bmp.width].join('|');
             var moved = structuredClone(bmp, {{ transfer: [bmp] }});
             var r2 = [moved.width, px(moved), bmp.width, bmp.height].join('|');
             var e1 = 'none'; try {{ structuredClone(bmp, {{ transfer: [bmp] }}); }} catch (x) {{ e1 = x.name; }}
             var e2 = 'none'; try {{ structuredClone(bmp); }} catch (x) {{ e2 = x.name; }}
             var o2 = new OffscreenCanvas(3, 4);
             var t2 = structuredClone({{ c: o2 }}, {{ transfer: [o2] }}).c;
             var r3 = [t2 instanceof OffscreenCanvas, t2.width, t2.height, o2.width, o2.height].join('|');
             var e3 = 'none'; try {{ o2.getContext('2d'); }} catch (x) {{ e3 = x.name; }}
             var e4 = 'none'; try {{ structuredClone(oc, {{ transfer: [oc] }}); }} catch (x) {{ e4 = x.name; }}
             var e5 = 'none'; try {{ structuredClone(new OffscreenCanvas(1, 1)); }} catch (x) {{ e5 = x.name; }}
             var rs = new ReadableStream();
             var e6 = 'none'; try {{ structuredClone(rs, {{ transfer: [rs] }}); }} catch (x) {{ e6 = x.name + ':' + /stream/.test(x.message); }}
             var viaPort = null; var ch = new MessageChannel();
             ch.port2.onmessage = function (e) {{ viaPort = [e.data.b instanceof ImageBitmap, e.data.b.width, px(e.data.b)].join('|'); }};
             var b3 = moved; ch.port1.postMessage({{ b: b3 }}, [b3]);
             var r4 = b3.width;
             var toWorker = null; var w = new Worker('{w}'); w.onmessage = function (e) {{ toWorker = e.data; }};
             var b5 = (function () {{ var c = new OffscreenCanvas(1, 1); c.getContext('2d').fillRect(0, 0, 1, 1); return c.transferToImageBitmap(); }})();
             w.postMessage(b5, [b5]);"
        ),
    );
    assert_eq!(eval(&mut p, "r1"), "true|2|1|255,0,0,255,0,0,255,255|2", "a cloned ImageBitmap keeps its pixels; the source stays open");
    assert_eq!(eval(&mut p, "r2"), "2|255,0,0,255,0,0,255,255|0|0", "a transferred ImageBitmap closes its source");
    assert_eq!(eval(&mut p, "[e1, e2].join('|')"), "DataCloneError|DataCloneError", "a closed ImageBitmap is neither transferable nor serializable");
    assert_eq!(eval(&mut p, "r3"), "true|3|4|0|0", "a transferred OffscreenCanvas: the size moves, the source is detached");
    assert_eq!(eval(&mut p, "[e3, e4, e5].join('|')"), "InvalidStateError|InvalidStateError|DataCloneError");
    assert_eq!(eval(&mut p, "e6"), "DataCloneError:true", "a ReadableStream is refused by name");
    assert_eq!(eval(&mut p, "r4"), "0");
    assert_eq!(run_until(&mut p, "viaPort !== null", "viaPort", 20), "true|2|255,0,0,255,0,0,255,255");
    assert_eq!(run_until(&mut p, "toWorker !== null", "toWorker", 20), "messageerror", "a worker realm has no canvas objects to receive");
}

// ------------------------------------------------------------------------- AETHERWORKERS3 (SR129) item 5

/// HTML §9.4.6 in a worker realm: forty MessageChannels nobody references are closed by the realm's collector
/// (FinalizationRegistry → `__port('collected')`) once garbage forces a collection; a referenced pair stays;
/// a port whose peer is the page's stays while that peer lives (the global holds it), and is collected
/// after the page closes its peer (`port-unentangled`). The counts are the registry's, for this realm.
#[test]
fn an_unreachable_port_pair_in_a_worker_is_closed_by_its_collector() {
    let w = data_url(
        "var junk = function () { var j = null; for (var i = 0; i < 300000; i++) { j = { a: i, b: [i] }; } return j; };
         var settle = function (want, tag, n) {
             junk();
             setTimeout(function () {
                 var live = __port('live');
                 if (live <= want || n > 40) { postMessage(tag + ':' + live); } else { settle(want, tag, n + 1); }
             }, 0);
         };
         onmessage = function (e) {
             if (e.data === 'start') {
                 self.keep = new MessageChannel();
                 (function () { for (var i = 0; i < 40; i++) { new MessageChannel(); } })();
                 postMessage('before:' + __port('live'));
                 settle(3, 'after', 0);
             } else if (e.data === 'again') {
                 settle(2, 'unentangled', 0);
             }
         };",
    );
    let mut p = page(
        PAGE,
        &format!(
            "var got = []; var w = new Worker('{w}'); w.onmessage = function (e) {{ got.push(e.data); }};
             var c = new MessageChannel(); w.postMessage('start', [c.port2]);"
        ),
    );
    assert_eq!(run_until(&mut p, "got.length >= 2", "got.join(',')", 60), "before:83,after:3", "the 80 unreachable ports are collected; keep and the cross-realm port stay");
    eval(&mut p, "c.port1.close(); w.postMessage('again');");
    assert_eq!(run_until(&mut p, "got.length >= 3", "got[2]", 60), "unentangled:2", "an unentangled port is collectable");
}

// ------------------------------------------------------------------------- AETHERWORKERS3 (SR129) item 6

/// Service Workers §3.4.4: `navigator.serviceWorker.ready` answers the same promise each time and resolves with
/// the registration once it is active. §4.4.2 / §4.3 in the worker: `clients.matchAll({ type })` filters by
/// client type ('worker' matches none of the window clients, 'all' with `includeUncontrolled` matches the
/// page, an invalid type rejects TypeError); the page as a WindowClient is 'visible', not focused, and its
/// `focus()` rejects InvalidAccessError (no user activation).
#[test]
fn service_worker_ready_is_one_promise_and_match_all_filters_by_type() {
    let port = server_ex(|path| match path {
        "/swc/sw.js" => Some(Fixture::js(
            "self.addEventListener('message', function (e) {
                 var c = e.source, out = [];
                 clients.matchAll({ type: 'worker' }).then(function (l) {
                     out.push('worker:' + l.length);
                     return clients.matchAll({ type: 'all', includeUncontrolled: true });
                 }).then(function (l) {
                     out.push('all:' + (l.length >= 1));
                     return clients.matchAll({ type: 'bogus' }).then(function () { out.push('bogus:ok'); }, function (x) { out.push('bogus:' + x.name); });
                 }).then(function () {
                     return clients.get(c.id);
                 }).then(function (wc) {
                     out.push('client:' + wc.type + ':' + wc.visibilityState + ':' + wc.focused);
                     return wc.focus().then(function () { out.push('focus:ok'); }, function (x) { out.push('focus:' + x.name); });
                 }).then(function () { c.postMessage(out.join(',')); });
             });",
        )),
        _ => None,
    });
    let mut p = page(
        &format!("http://127.0.0.1:{port}/swc/page"),
        "var c0 = navigator.serviceWorker.controller;
         var same = navigator.serviceWorker.ready === navigator.serviceWorker.ready;
         var got = null, readyScope = null;
         navigator.serviceWorker.onmessage = function (e) { got = e.data; };
         navigator.serviceWorker.register('/swc/sw.js').then(function () { return navigator.serviceWorker.ready; })
             .then(function (r) { readyScope = r.scope; r.active.postMessage('go'); });",
    );
    assert_eq!(eval(&mut p, "same"), "true", "ready is one promise");
    assert_eq!(run_until(&mut p, "readyScope !== null", "readyScope", 20), format!("http://127.0.0.1:{port}/swc/"));
    assert_eq!(run_until(&mut p, "got !== null", "got", 20), "worker:0,all:true,bogus:TypeError,client:window:visible:false,focus:InvalidAccessError");
}
