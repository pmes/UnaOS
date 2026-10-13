//! AETHERJS2 (LEDGER SR106): the page-level half of the 2026 JavaScript surface, each test asserting the
//! observable a page reads back — import maps through every module path (HTML §8.1.5.3/§8.1.5.5), the
//! event loop's ordering rules, one test per rule (HTML §8.1.7.3: microtasks after each task and script,
//! timers by due time, animation frames with one timestamp, microtasks between frame callbacks, in-frame
//! cancellation, `unhandledrejection` / `rejectionhandled` tasks per §8.1.5.7, the Prioritized Task
//! Scheduling queues), `structuredClone` with `transfer`,
//! `AbortSignal` across `fetch`, the inline event handler scope chain (§8.1.8.1) with [Unscopable], and Intl
//! inside a page. Expected values are Chromium's for the same pages.

use std::path::PathBuf;

fn load(url: &str, html: &str, budget_ms: u64) -> aether::AetherEngine {
    let mut engine = aether::AetherEngine::new();
    engine.load_html(url, html, true);
    if let Some(js) = engine.js_engine.as_mut() {
        aether::event_loop::settle(js, budget_ms);
    }
    engine
}

fn eval(engine: &mut aether::AetherEngine, src: &str) -> String {
    let js = engine.js_engine.as_mut().expect("page has a script engine");
    js.eval_string(src).unwrap_or_else(|e| format!("THROW {e}"))
}

fn scratch(name: &str) -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("aetherjs2").join(name);
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// HTML §8.1.5.5–6: `<script type="importmap">` maps bare specifiers (exact and "/" prefix keys), scopes
/// override by referrer, and the map serves static imports, dynamic `import()` from classic and module
/// scripts, top-level await and `import.meta.resolve`; an unmapped bare specifier is a TypeError.
#[test]
fn import_maps_resolve_every_module_path() {
    let dir = scratch("importmap");
    for (rel, src) in [
        ("lib/index.js", "export const name = 'lib'; export default 41;"),
        ("lib/util.js", "export const u = 'util';"),
        ("lib/alt.js", "export default 'alt';"),
        ("scoped/s.js", "import alt from 'lib'; export const fromScope = alt;"),
    ] {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, src).unwrap();
    }
    let html = r#"<!doctype html><html><head>
<script type="importmap">
{ "imports": { "lib": "./lib/index.js", "lib/": "./lib/" },
  "scopes": { "./scoped/": { "lib": "./lib/alt.js" } } }
</script>
<script>
var res = {};
import('lib').then(function (m) { res.classic = m.name; });
import('nope').catch(function (e) { res.bare = e.name; });
</script>
<script type="module">
import v, { name } from 'lib';
import { u } from 'lib/util.js';
import { fromScope } from './scoped/s.js';
res.module = [v + 1, name, u, fromScope].join();
res.meta = import.meta.resolve('lib/util.js') === new URL('./lib/util.js', document.baseURI).href;
res.tla = (await import('lib')).name;
</script>
</head><body></body></html>"#;
    let url = format!("file://{}/page.html", dir.display());
    let mut e = load(&url, html, 2_000);
    assert_eq!(eval(&mut e, "[res.classic, res.bare, res.module, res.meta, res.tla].join('|')"), "lib|TypeError|42,lib,util,alt|true|lib");
}

fn page(script: &str) -> String {
    format!("<!doctype html><html><body><script>{script}</script></body></html>")
}

/// HTML §8.1.7.3 rule 1: the microtask checkpoint after a script runs every promise job and
/// `queueMicrotask` callback, in FIFO order, before any task (here a 0 ms timer).
#[test]
fn event_loop_microtasks_before_the_next_task() {
    let mut e = load(
        "http://ordering.test/1",
        &page("var log = []; setTimeout(function () { log.push('t'); }, 0); Promise.resolve().then(function () { log.push('m1'); }); queueMicrotask(function () { log.push('m2'); }); log.push('s');"),
        500,
    );
    assert_eq!(eval(&mut e, "log.join()"), "s,m1,m2,t");
}

/// Rule 2: "clean up after running script" checkpoints between two parser-inserted scripts.
#[test]
fn event_loop_checkpoint_between_scripts() {
    let html = "<!doctype html><html><body><script>var log = []; Promise.resolve().then(function () { log.push('mA'); });</script><script>log.push('B');</script></body></html>";
    let mut e = load("http://ordering.test/2", html, 500);
    assert_eq!(eval(&mut e, "log.join()"), "mA,B");
}

/// Rule 3 (HTML §8.6 timers): timers run in due order, ties in insertion order, each its own task with
/// its own checkpoint (a microtask a timer queues runs before the next timer).
#[test]
fn event_loop_timers_are_tasks_in_due_order() {
    let mut e = load(
        "http://ordering.test/3",
        &page("var log = []; setTimeout(function () { log.push('late'); }, 50); setTimeout(function () { log.push('t1'); Promise.resolve().then(function () { log.push('t1m'); }); }, 0); setTimeout(function () { log.push('t2'); }, 0);"),
        500,
    );
    assert_eq!(eval(&mut e, "log.join()"), "t1,t1m,t2,late");
}

/// Rule 4 (HTML "run the animation frame callbacks"): one timestamp for the frame, microtasks between
/// callbacks, a sibling canceled by an earlier callback is skipped, a callback registered during the frame
/// runs in the next one.
#[test]
fn event_loop_animation_frame_rules() {
    let mut e = load(
        "http://ordering.test/4",
        &page("var raf = [], t1, id2; requestAnimationFrame(function (ts) { raf.push('r1'); t1 = ts; Promise.resolve().then(function () { raf.push('r1m'); }); requestAnimationFrame(function () { raf.push('r3'); }); cancelAnimationFrame(id2); }); id2 = requestAnimationFrame(function () { raf.push('r2'); }); requestAnimationFrame(function (ts) { raf.push(ts === t1 ? 'r4same' : 'r4diff'); });"),
        500,
    );
    assert_eq!(eval(&mut e, "raf.join()"), "r1,r1m,r4same,r3");
}

/// Rule 5 (HTML §8.1.5.7): a rejection still unhandled after the checkpoint is notified by a task
/// (`unhandledrejection`, cancelable — canceled, nothing reaches the console) that runs before the timers
/// of the same turn; handling it later queues `rejectionhandled`.
#[test]
fn event_loop_rejection_notification_tasks() {
    let mut e = load(
        "http://ordering.test/5",
        &page("var log = []; addEventListener('unhandledrejection', function (e) { log.push('ur:' + e.reason); e.preventDefault(); }); addEventListener('rejectionhandled', function (e) { log.push('rh:' + e.reason); }); var p = Promise.reject('y'); setTimeout(function () { log.push('t'); p.catch(function () {}); }, 0); setTimeout(function () { log.push('t2'); }, 20);"),
        500,
    );
    assert_eq!(eval(&mut e, "log.join()"), "ur:y,t,rh:y,t2");
    assert!(!aether::js::console_lines().iter().any(|(_, l)| l.contains("Uncaught (in promise)")));
}

/// Rule 6 (WICG Prioritized Task Scheduling): user-blocking before user-visible before background, a
/// yield() continuation ahead of its priority's tasks, setPriority moving a queued task, abort rejecting.
#[test]
fn event_loop_scheduler_priorities() {
    let mut e = load(
        "http://ordering.test/6",
        &page("var sch = []; scheduler.postTask(function () { sch.push('bg'); }, { priority: 'background' }); scheduler.postTask(function () { sch.push('uv'); }); scheduler.postTask(function () { sch.push('ub'); }, { priority: 'user-blocking' }); (async function () { sch.push('y0'); await scheduler.yield(); sch.push('y1'); })(); var tc = new TaskController({ priority: 'background' }); scheduler.postTask(function () { sch.push('moved'); }, { signal: tc.signal }); tc.setPriority('user-blocking'); var ac = new AbortController(); scheduler.postTask(function () { sch.push('never'); }, { signal: ac.signal }).catch(function (err) { sch.push(err.name); }); ac.abort();"),
        500,
    );
    assert_eq!(eval(&mut e, "sch.join()"), "y0,AbortError,ub,moved,y1,uv,bg");
    assert_eq!(eval(&mut e, "tc.signal.priority + ',' + (tc.signal instanceof AbortSignal)"), "user-blocking,true");
}

/// HTML §2.7 structuredClone (graph, types, DataCloneError, `transfer`) and DOM §3.1 AbortSignal through
/// `fetch`, `AbortSignal.any` and `AbortSignal.timeout`.
#[test]
fn structured_clone_and_abort_signals() {
    let html = r#"<!doctype html><html><body><script>
var o = { d: new Date(0), m: new Map([[1, { x: 1 }]]), s: new Set(['a']), r: /a/gi, u8: new Uint8Array([1, 2]), big: 10n, e: new RangeError('bad') };
o.self = o;
var c = structuredClone(o);
var sc = [c !== o, c.self === c, c.d.getTime(), c.m.get(1).x, c.s.has('a'), c.r.flags, c.u8[1], typeof c.big, c.e instanceof RangeError, c.e.message].join();
var dce; try { structuredClone(function f() {}); } catch (err) { dce = err.name; }
var buf = new ArrayBuffer(8);
var moved = structuredClone({ b: buf }, { transfer: [buf] });
var tr = [buf.byteLength, moved.b.byteLength].join();
var ab = [];
fetch('http://127.0.0.1:9/never', { signal: AbortSignal.abort() }).catch(function (err) { ab.push(err.name); });
var ctl = new AbortController();
ctl.signal.addEventListener('abort', function () { ab.push('ev:' + ctl.signal.reason); });
var any = AbortSignal.any([ctl.signal]);
ctl.abort('why');
ab.push('any:' + any.aborted);
var ts = AbortSignal.timeout(5);
ts.onabort = function () { ab.push(ts.reason.name); };
</script></body></html>"#;
    let mut e = load("http://clone.test/", html, 1_000);
    assert_eq!(eval(&mut e, "sc"), "true,true,0,1,true,gi,2,bigint,true,bad");
    assert_eq!(eval(&mut e, "dce"), "DataCloneError");
    assert_eq!(eval(&mut e, "tr"), "0,8");
    assert_eq!(eval(&mut e, "ab.join()"), "ev:why,any:true,AbortError,TimeoutError");
}

/// HTML §8.1.8.1: an inline handler sees its element, then its form owner, then its document (as `with`
/// scopes), never their [Unscopable] members (WebIDL §3.7.3).
#[test]
fn inline_handler_scope_chain_and_unscopables() {
    let html = r#"<!doctype html><html><body>
<form id="f"><input id="i" name="q" value="v" onclick="window.r = [value, name, typeof elements, typeof append, typeof URL].join()"></form>
</body></html>"#;
    let mut e = load("http://handlers.test/", html, 500);
    assert_eq!(eval(&mut e, "document.getElementById('i').click(); window.r"), "v,q,object,undefined,string");
    assert_eq!(eval(&mut e, "document.getElementById('i').onclick.name"), "onclick");
    assert_eq!(
        eval(&mut e, "var u = Element.prototype[Symbol.unscopables]; [Object.getPrototypeOf(u), u.append, u.slot, Document.prototype[Symbol.unscopables].prepend].join()"),
        ",true,true,true"
    );
}

/// ECMA-402 inside a page: the realm Aether builds carries Intl, and the locale-sensitive methods use it.
#[test]
fn intl_in_a_page() {
    let html = "<!doctype html><html><body><script>var out = [(1234.5).toLocaleString(), new Intl.DateTimeFormat().resolvedOptions().locale, new Date(Date.UTC(2024, 0, 15)).toLocaleDateString('en-US', {timeZone: 'UTC', month: 'short', day: 'numeric'}), new Intl.PluralRules('en', {type: 'ordinal'}).select(22)].join('|');</script></body></html>";
    let mut e = load("http://intl.test/", html, 500);
    assert_eq!(eval(&mut e, "out"), "1,234.5|en-US|Jan 15|two");
}
