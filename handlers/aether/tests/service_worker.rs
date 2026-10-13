//! AETHERSW (LEDGER SR104): service workers that control pages, one host test per rule, each against its own
//! local fixture server on 127.0.0.1:0 (its own origin, so its own storage key and registrations):
//!
//! 1. a navigation under a registered scope is the worker's `fetch` event and `respondWith` resolves it;
//! 2. a worker that does not respond lets the navigation fall through to the network;
//! 3. a controlled document's subresources (the page loader's sheet and script) go to the same worker;
//! 4. `clients.claim()` takes an open in-scope page, which hears `controllerchange` and whose `fetch()` the
//!    worker then answers;
//! 5. install → waiting → active: an update while a page is controlled WAITS, is promoted when the page goes,
//!    and `skipWaiting()` takes the controlled pages over at once;
//! 6. the update check on navigation byte-compares the script: identical bytes install nothing, changed bytes
//!    install (and, nothing being controlled, activate) the new worker;
//! 7. the worker reads and writes the SR94 Cache API: what its `install` stored, the page reads back, and its
//!    `fetch` answers from it without the network;
//! 8. `postMessage` page → worker → page;
//! 9. offline: the network refuses the navigation and the worker answers from its cache;
//! 10. policy (`security::refusal`): an opaque answer to a navigation or a `cors` request is refused, a `no-cors`
//!     request may receive one, a cross-origin `cors` fetch from the worker is CORS-checked.
//!
//! No WPT: the seat runs the suites. Storage lives under CARGO_TARGET_TMPDIR (one directory per test process).

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A fixture's answer: a response, or a dropped connection (the network refuses).
enum Reply {
    Ok(u16, &'static str, Vec<u8>),
    Hdr(u16, Vec<(String, String)>, Vec<u8>),
    Drop,
}

fn ok(ctype: &'static str, body: impl Into<Vec<u8>>) -> Reply {
    Reply::Ok(200, ctype, body.into())
}

#[derive(Default)]
struct State {
    /// Requests per path.
    hits: Mutex<HashMap<String, usize>>,
    /// The script version a fixture serves (tests 5 and 6 bump it).
    version: AtomicUsize,
}

impl State {
    fn hits(&self, path: &str) -> usize {
        self.hits.lock().unwrap().get(path).copied().unwrap_or(0)
    }
}

type Route = Arc<dyn Fn(&str, &State) -> Reply + Send + Sync>;

async fn handle(mut s: TcpStream, st: Arc<State>, route: Route) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 8192];
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        match s.read(&mut tmp).await {
            Ok(0) | Err(_) => return,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf).to_string();
    let target = head.split(' ').nth(1).unwrap_or("/").to_string();
    let path = target.split('?').next().unwrap_or("/").to_string();
    *st.hits.lock().unwrap().entry(path.clone()).or_default() += 1;
    let (status, mut headers, body) = match route(&target, &st) {
        Reply::Drop => return,
        Reply::Ok(code, ctype, body) => (code, vec![("Content-Type".to_string(), ctype.to_string())], body),
        Reply::Hdr(code, h, body) => (code, h, body),
    };
    headers.push(("Cache-Control".into(), "no-store".into()));
    let mut h = format!("HTTP/1.1 {status} X\r\nContent-Length: {}\r\nConnection: close\r\n", body.len());
    for (k, v) in &headers {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    let _ = s.write_all(h.as_bytes()).await;
    let _ = s.write_all(&body).await;
    let _ = s.flush().await;
}

/// A fixture server; `route(target, state)` answers each request (the target keeps its query).
fn server(route: impl Fn(&str, &State) -> Reply + Send + Sync + 'static) -> (u16, Arc<State>) {
    init();
    let st = Arc::new(State::default());
    let route: Route = Arc::new(route);
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
                tokio::spawn(handle(c, st2.clone(), route.clone()));
            }
        });
    });
    (rx.recv().unwrap(), st)
}

/// Every test process keeps its storage (registrations, caches) in its own directory under the target dir.
fn init() {
    static ONCE: OnceLock<()> = OnceLock::new();
    ONCE.get_or_init(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("aether-sw-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        aether::storage::set_root_dir(Some(dir));
    });
}

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap())
}

/// A top-level navigation through the page loader's document fetch: (html, final URL) or the error text.
fn navigate(url: &str) -> Result<(String, String), String> {
    rt().block_on(aether::net::fetch_document_final(url)).map_err(|e| format!("{e:#}"))
}

fn page(url: &str, script: &str) -> aether::AetherEngine {
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

fn eval(engine: &mut aether::AetherEngine, src: &str) -> String {
    engine.js_engine.as_mut().expect("engine").eval_string(src).unwrap_or_else(|e| format!("<error {e}>"))
}

/// The registering page's script: registers `/app/sw.js` for `/app/`, keeps the registration as `REG`.
const REGISTER: &str = r#"
    var out = null, REG = null;
    navigator.serviceWorker.register('/app/sw.js', { scope: '/app/' }).then(function (r) {
        REG = r; out = 'ok';
    }, function (e) { out = 'err:' + e.name + ':' + e.message; });
"#;

fn register(origin: &str) -> aether::AetherEngine {
    let mut e = page(&format!("{origin}/page"), REGISTER);
    assert_eq!(run_until(&mut e, "out !== null", "out", 30), "ok", "the registration settles");
    e
}

fn wait_for(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(secs);
    while !f() {
        assert!(Instant::now() < end, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(10));
    }
}

const JS: &str = "text/javascript";
const HTML: &str = "text/html";

// ----------------------------------------------------------------------------------------------------- 1

#[test]
fn navigation_is_answered_by_respond_with() {
    let (port, st) = server(|t, _| match t {
        "/app/sw.js" => ok(
            JS,
            r#"self.addEventListener('fetch', function (e) {
                if (e.request.url.indexOf('/app/hello') >= 0) {
                    e.respondWith(new Response('<!doctype html><p>from-worker ' + e.request.mode + ' ' + e.request.destination + ' ' + e.clientId.length + '</p>',
                        { headers: { 'Content-Type': 'text/html' } }));
                }
            });"#,
        ),
        _ => ok(HTML, format!("network:{t}")),
    });
    let origin = format!("http://127.0.0.1:{port}");
    let _reg = register(&origin);
    let (html, url) = navigate(&format!("{origin}/app/hello")).expect("the worker's document");
    assert!(html.contains("from-worker navigate document 0"), "{html}");
    assert_eq!(url, format!("{origin}/app/hello"), "a constructed Response's document keeps the request URL");
    assert_eq!(st.hits("/app/hello"), 0, "the network never saw the navigation");
    // Outside the scope there is no worker.
    assert_eq!(navigate(&format!("{origin}/elsewhere")).unwrap().0, "network:/elsewhere");
}

// ----------------------------------------------------------------------------------------------------- 2

#[test]
fn navigation_without_respond_with_falls_through_to_the_network() {
    let (port, st) = server(|t, _| match t {
        "/app/sw.js" => ok(JS, "var seen = 0; self.addEventListener('fetch', function (e) { seen++; /* no respondWith */ });"),
        _ => ok(HTML, format!("network:{t}")),
    });
    let origin = format!("http://127.0.0.1:{port}");
    let _reg = register(&origin);
    let (html, url) = navigate(&format!("{origin}/app/plain")).expect("the network's document");
    assert_eq!(html, "network:/app/plain");
    assert_eq!(url, format!("{origin}/app/plain"));
    assert_eq!(st.hits("/app/plain"), 1, "exactly one network request: the fall-through");
}

// ----------------------------------------------------------------------------------------------------- 3

#[test]
fn a_controlled_documents_subresources_go_to_its_worker() {
    let (port, st) = server(|t, _| match t {
        "/app/sw.js" => ok(
            JS,
            r#"self.addEventListener('fetch', function (e) {
                if (e.request.url.endsWith('/app/style.css')) {
                    e.respondWith(new Response('p { color: rgb(1, 2, 3) } /* ' + e.request.mode + ' ' + e.request.destination + ' */',
                        { headers: { 'Content-Type': 'text/css' } }));
                }
                // a.js: no respondWith — the network serves it.
            });"#,
        ),
        "/app/doc" => ok(HTML, r#"<!doctype html><link rel="stylesheet" href="/app/style.css"><script src="/app/a.js"></script><p>doc</p>"#),
        "/app/a.js" => ok(JS, "var fromNetwork = 1;"),
        "/app/style.css" => ok("text/css", "p { color: red }"),
        _ => ok(HTML, "x"),
    });
    let origin = format!("http://127.0.0.1:{port}");
    let _reg = register(&origin);
    let page = rt().block_on(aether::net::fetch_page(&format!("{origin}/app/doc"))).expect("the page loads");
    assert_eq!(page.sheets.len(), 1);
    assert!(page.sheets[0].contains("rgb(1, 2, 3)") && page.sheets[0].contains("no-cors style"), "{:?}", page.sheets);
    assert_eq!(st.hits("/app/style.css"), 0, "the worker answered the sheet");
    assert!(page.scripts.iter().any(|s| s.contains("fromNetwork")), "{:?}", page.scripts);
    assert_eq!(st.hits("/app/a.js"), 1, "the unanswered script fell through to the network once");
    assert_eq!(st.hits("/app/doc"), 1, "the worker let the navigation through");
}

// ----------------------------------------------------------------------------------------------------- 4

#[test]
fn clients_claim_takes_an_open_page() {
    let (port, st) = server(|t, _| match t {
        "/app/sw.js" => ok(
            JS,
            r#"self.addEventListener('activate', function (e) { e.waitUntil(clients.claim()); });
               self.addEventListener('fetch', function (e) {
                   if (e.request.url.endsWith('/app/data')) { e.respondWith(new Response('claimed:' + e.request.mode)); }
               });"#,
        ),
        _ => ok("text/plain", format!("network:{t}")),
    });
    let origin = format!("http://127.0.0.1:{port}");
    let mut e = page(
        &format!("{origin}/app/page"),
        r#"
        var log = [String(navigator.serviceWorker.controller)], got = null;
        navigator.serviceWorker.oncontrollerchange = function () {
            log.push('controllerchange');
            log.push(navigator.serviceWorker.controller.scriptURL.replace(location.origin, ''));
            fetch('/app/data').then(function (r) { return r.text(); }).then(function (t) { got = t; }, function (e) { got = 'err:' + e; });
        };
        navigator.serviceWorker.register('/app/sw.js', { scope: '/app/' });
    "#,
    );
    let out = run_until(&mut e, "got !== null", "log.join('|') + '|' + got", 30);
    assert_eq!(out, "null|controllerchange|/app/sw.js|claimed:cors");
    assert_eq!(st.hits("/app/data"), 0, "the claimed page's fetch went to the worker");
}

// ----------------------------------------------------------------------------------------------------- 5

fn versioned(t: &str, st: &State) -> Reply {
    match t {
        "/app/sw.js" => {
            let v = st.version.load(Ordering::SeqCst);
            let skip = if v >= 3 { "self.addEventListener('install', function () { self.skipWaiting(); });" } else { "" };
            ok(
                JS,
                format!(
                    "var V = 'v{v}'; {skip}
                     self.addEventListener('install', function (e) {{ e.waitUntil(fetch('/ping-install?v=' + V)); }});
                     self.addEventListener('fetch', function (e) {{ if (e.request.url.endsWith('/app/ver')) {{ e.respondWith(new Response(V)); }} }});"
                ),
            )
        }
        _ => ok("text/plain", format!("network:{t}")),
    }
}

/// A controlled page on its own thread (its own document): it reads its controller and fetches `/app/ver`,
/// reports `controlled:<ver>`, then waits for `go`; with `after_change` it then waits for a `controllerchange`
/// and the `/app/ver` its new controller answers (`<changes>:<ver>`). The thread's end retires the document.
fn controlled_page(url: String, ready: std::sync::mpsc::Sender<String>, go: std::sync::mpsc::Receiver<()>, after_change: bool) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut e = page(
            &url,
            r#"
            var cc = 0, ver = null, ver2 = null;
            var first = navigator.serviceWorker.controller ? 'controlled' : 'none';
            navigator.serviceWorker.oncontrollerchange = function () {
                cc++;
                fetch('/app/ver').then(function (r) { return r.text(); }).then(function (t) { ver2 = t; });
            };
            fetch('/app/ver').then(function (r) { return r.text(); }).then(function (t) { ver = t; });
        "#,
        );
        let first = run_until(&mut e, "ver !== null", "first + ':' + ver", 30);
        ready.send(first).unwrap();
        go.recv().unwrap();
        if after_change { run_until(&mut e, "ver2 !== null", "cc + ':' + ver2", 30) } else { String::new() }
    })
}

#[test]
fn install_waiting_active_and_skip_waiting() {
    let (port, st) = server(versioned);
    st.version.store(1, Ordering::SeqCst);
    let origin = format!("http://127.0.0.1:{port}");
    let mut reg = register(&origin);
    assert_eq!(st.hits("/ping-install"), 1, "v1 installed");

    // B is controlled by v1.
    let (rtx, rrx) = std::sync::mpsc::channel();
    let (gtx, grx) = std::sync::mpsc::channel();
    let b = controlled_page(format!("{origin}/app/b"), rtx, grx, false);
    assert_eq!(rrx.recv().unwrap(), "controlled:v1");

    // v2 installs and WAITS while B is controlled.
    st.version.store(2, Ordering::SeqCst);
    eval(&mut reg, "var up = null; REG.update().then(function () { up = (REG.waiting ? 'waiting' : 'none') + ':' + (REG.active ? 'active' : '-'); }, function (e) { up = 'err:' + e.message; })");
    assert_eq!(run_until(&mut reg, "up !== null", "up", 30), "waiting:active");
    assert_eq!(st.hits("/ping-install"), 2, "v2 ran install");
    let regs = aether::workers::sw::registrations(&origin);
    assert!(regs[0]["waiting"].as_str().is_some_and(|w| w.ends_with("/app/sw.js")), "{regs}");

    // B goes: the waiting worker is promoted.
    gtx.send(()).unwrap();
    b.join().unwrap();
    wait_for("the waiting worker's promotion", 20, || aether::workers::sw::registrations(&origin)[0]["waiting"].is_null());
    assert_eq!(navigate(&format!("{origin}/app/ver")).unwrap().0, "v2", "v2 is active");

    // C is controlled by v2; v3 calls skipWaiting() and takes C over at once.
    let (rtx, rrx) = std::sync::mpsc::channel();
    let (gtx, grx) = std::sync::mpsc::channel();
    let c = controlled_page(format!("{origin}/app/c"), rtx, grx, true);
    assert_eq!(rrx.recv().unwrap(), "controlled:v2");
    st.version.store(3, Ordering::SeqCst);
    eval(&mut reg, "up = null; REG.update().then(function () { up = (REG.waiting ? 'waiting' : 'none') + ':' + (REG.active ? 'active' : '-'); }, function (e) { up = 'err:' + e.message; })");
    assert_eq!(run_until(&mut reg, "up !== null", "up", 30), "none:active", "skipWaiting activated v3 despite a controlled page");
    gtx.send(()).unwrap();
    assert_eq!(c.join().unwrap(), "1:v3", "C heard controllerchange and is served by v3");
}

// ----------------------------------------------------------------------------------------------------- 6

#[test]
fn the_update_check_on_navigation_byte_compares_the_script() {
    let (port, st) = server(versioned);
    st.version.store(1, Ordering::SeqCst);
    let origin = format!("http://127.0.0.1:{port}");
    let _reg = register(&origin);
    assert_eq!((st.hits("/app/sw.js"), st.hits("/ping-install")), (1, 1));

    // Identical bytes: the navigation's update check fetches the script and installs nothing.
    assert_eq!(navigate(&format!("{origin}/app/ver")).unwrap().0, "v1");
    wait_for("the update check's script fetch", 20, || st.hits("/app/sw.js") >= 2);
    std::thread::sleep(Duration::from_millis(500));
    assert_eq!(st.hits("/ping-install"), 1, "a byte-identical script is no update");

    // Changed bytes: the next navigation's check installs v2 (nothing is controlled: it activates).
    st.version.store(2, Ordering::SeqCst);
    let mut last = String::new();
    wait_for("v2 to answer the navigation", 30, || {
        last = navigate(&format!("{origin}/app/ver")).unwrap().0;
        last == "v2"
    });
    assert_eq!(st.hits("/ping-install"), 2, "v2 installed exactly once");
    let lu = aether::workers::sw::registrations(&origin);
    assert!(lu[0]["waiting"].is_null(), "{lu}");
}

// ----------------------------------------------------------------------------------------------------- 7

#[test]
fn the_worker_reads_and_writes_the_cache_api() {
    let (port, st) = server(|t, _| match t {
        "/app/sw.js" => ok(
            JS,
            r#"self.addEventListener('install', function (e) {
                e.waitUntil(caches.open('v1').then(function (c) {
                    return c.put(new Request(registration.scope + 'cached'), new Response('from-cache', { headers: { 'Content-Type': 'text/plain' } }))
                        .then(function () { return c.addAll([registration.scope + 'asset.txt']); });
                }));
            });
            self.addEventListener('fetch', function (e) {
                e.respondWith(caches.match(e.request).then(function (r) { return r || fetch(e.request); }));
            });"#,
        ),
        "/app/asset.txt" => ok("text/plain", "asset-body"),
        _ => ok(HTML, format!("network:{t}")),
    });
    let origin = format!("http://127.0.0.1:{port}");
    let mut reg = register(&origin);
    assert_eq!(st.hits("/app/asset.txt"), 1, "addAll fetched the asset once");
    // The page (same storage key) reads what the worker stored.
    eval(
        &mut reg,
        "var rd = null; caches.keys().then(function (k) { return caches.open('v1').then(function (c) { return c.match(location.origin + '/app/cached'); }).then(function (r) { return r.text(); }).then(function (t) { rd = k.join(',') + '|' + t; }); }).catch(function (e) { rd = 'err:' + e; })",
    );
    assert_eq!(run_until(&mut reg, "rd !== null", "rd", 20), "v1|from-cache");
    // The worker answers from its cache: no second network request.
    assert_eq!(navigate(&format!("{origin}/app/asset.txt")).unwrap().0, "asset-body");
    assert_eq!(st.hits("/app/asset.txt"), 1, "served from the Cache API");
    // A miss passes through to the network.
    assert_eq!(navigate(&format!("{origin}/app/miss")).unwrap().0, "network:/app/miss");
}

// ----------------------------------------------------------------------------------------------------- 8

#[test]
fn post_message_between_a_page_and_its_worker() {
    let (port, _st) = server(|t, _| match t {
        "/app/sw.js" => ok(
            JS,
            r#"self.addEventListener('message', function (e) {
                e.source.postMessage({ echo: e.data.n * 2, sameOrigin: e.origin === registration.scope.replace(/\/app\/$/, ''), kind: typeof e.source.id });
            });"#,
        ),
        _ => ok(HTML, "x"),
    });
    let origin = format!("http://127.0.0.1:{port}");
    let mut reg = register(&origin);
    eval(
        &mut reg,
        "var msg = null; navigator.serviceWorker.onmessage = function (ev) { msg = ev.data.echo + '|' + ev.data.sameOrigin + '|' + ev.data.kind + '|' + (ev.origin === location.origin); }; REG.active.postMessage({ n: 21 });",
    );
    assert_eq!(run_until(&mut reg, "msg !== null", "msg", 20), "42|true|string|true");
}

// ----------------------------------------------------------------------------------------------------- 9

#[test]
fn offline_the_worker_answers_from_its_cache() {
    let (port, st) = server(|t, _| match t {
        "/app/sw.js" => ok(
            JS,
            r#"self.addEventListener('install', function (e) {
                e.waitUntil(caches.open('offline').then(function (c) { return c.addAll([registration.scope + 'offline.html']); }));
            });
            self.addEventListener('fetch', function (e) {
                if (e.request.mode === 'navigate') {
                    e.respondWith(fetch(e.request).catch(function () { return caches.match(registration.scope + 'offline.html'); }));
                }
            });"#,
        ),
        "/app/offline.html" => ok(HTML, "<!doctype html><p>offline-page</p>"),
        "/app/live" => Reply::Drop,
        _ => ok(HTML, format!("network:{t}")),
    });
    let origin = format!("http://127.0.0.1:{port}");
    let _reg = register(&origin);
    let (html, _) = navigate(&format!("{origin}/app/live")).expect("the cached offline page");
    assert!(html.contains("offline-page"), "{html}");
    assert!(st.hits("/app/live") >= 1, "the worker tried the network first");
    // Online, the same worker passes the network's answer through.
    assert_eq!(navigate(&format!("{origin}/app/up")).unwrap().0, "network:/app/up");
}

// ---------------------------------------------------------------------------------------------------- 10

#[test]
fn policy_refuses_opaque_answers_to_navigations_and_cors_requests() {
    let (other, _ost) = server(|t, _| match t {
        "/cors.txt" => Reply::Hdr(200, vec![("Content-Type".into(), "text/plain".into()), ("Access-Control-Allow-Origin".into(), "*".into())], b"cors-body".to_vec()),
        _ => ok("text/plain", "opaque-bytes"),
    });
    let other_origin = format!("http://127.0.0.1:{other}");
    let oo = other_origin.clone();
    let (port, _st) = server(move |t, _| match t {
        "/app/sw.js" => ok(
            JS,
            format!(
                r#"var O = '{oo}';
                self.addEventListener('activate', function (e) {{ e.waitUntil(clients.claim()); }});
                self.addEventListener('fetch', function (e) {{
                    var u = e.request.url;
                    if (u.indexOf('/app/opaque') >= 0) {{ e.respondWith(fetch(O + '/img.png', {{ mode: 'no-cors' }})); }}
                    else if (u.endsWith('/app/cors-ok')) {{ e.respondWith(fetch(O + '/cors.txt')); }}
                    else if (u.endsWith('/app/cors-refused')) {{ e.respondWith(fetch(O + '/img.png')); }}
                }});"#
            ),
        ),
        _ => ok(HTML, format!("network:{t}")),
    });
    let origin = format!("http://127.0.0.1:{port}");

    // A controlled page (claimed): its fetch() under each mode.
    let mut e = page(
        &format!("{origin}/app/page"),
        r#"
        var res = {}, n = 0;
        var probe = function (name, url, init) {
            fetch(url, init).then(function (r) {
                return r.text().then(function (t) { res[name] = r.type + ':' + r.status + ':' + t; });
            }, function (e) { res[name] = e.name; }).then(function () { n++; });
        };
        String(navigator.serviceWorker.controller);
        navigator.serviceWorker.oncontrollerchange = function () {
            probe('nocors', '/app/opaque-sub', { mode: 'no-cors' });
            probe('cors', '/app/opaque-sub');
            probe('corsok', '/app/cors-ok');
            probe('refused', '/app/cors-refused');
        };
        navigator.serviceWorker.register('/app/sw.js', { scope: '/app/' });
    "#,
    );
    let out = run_until(&mut e, "n === 4", "[res.nocors, res.cors, res.corsok, res.refused].join('|')", 30);
    assert_eq!(out, "opaque:0:|TypeError|cors:200:cors-body|TypeError");

    // A navigation answered with an opaque response is a network error, refused through security/.
    let err = navigate(&format!("{origin}/app/opaque-nav")).expect_err("an opaque navigation answer is refused");
    assert!(err.contains("opaque"), "{err}");
    let refusals = aether::security::refusals();
    assert!(refusals.iter().any(|r| r.starts_with("security-sw-opaque-response")), "{refusals:?}");
    assert!(refusals.iter().any(|r| r.starts_with("security-sw-cors")), "{refusals:?}");
}
