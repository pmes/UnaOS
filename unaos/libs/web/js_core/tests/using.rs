//! USINGSYNTAX (LEDGER SR114): the Explicit Resource Management declarations (ES2026 §14.3.1 `using` /
//! `await using`, §14.7 for heads, DisposeResources) over the library half AETHERJS2 (SR106) landed. Each case
//! is a proposal example or one of its early errors; the expected strings are what the specification makes the
//! program print (the shapes of test262's language/statements/using and await-using directories).

use js_core::vm::{NullHost, Value, Vm};

fn show(vm: &mut Vm, v: Result<Value, Value>) -> String {
    match v {
        Ok(Value::String(s)) => s.to_rust(),
        Ok(other) => vm.error_string(&other),
        Err(e) => format!("THROW {}", vm.error_string(&e)),
    }
}

fn check(cases: &[(&str, &str)]) {
    let mut fails = Vec::new();
    for (src, want) in cases {
        let mut vm = Vm::new(Box::new(NullHost));
        let r = vm.run_script_str(src);
        let got = show(&mut vm, r);
        if got != *want {
            fails.push(format!("{src:?}: got {got:?}, want {want:?}"));
        }
    }
    for f in &fails {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} failures", fails.len());
}

/// Run `setup`, drain the job queue, then evaluate `expr`.
fn after_jobs(setup: &str, expr: &str) -> String {
    let mut vm = Vm::new(Box::new(NullHost));
    if let Err(e) = vm.run_script_str(setup) {
        return format!("THROW {}", vm.error_string(&e));
    }
    let _ = vm.run_jobs();
    let r = vm.run_script_str(expr);
    show(&mut vm, r)
}

fn check_async(cases: &[(&str, &str, &str)]) {
    let mut fails = Vec::new();
    for (setup, expr, want) in cases {
        let got = after_jobs(setup, expr);
        if got != *want {
            fails.push(format!("{setup:?}: got {got:?}, want {want:?}"));
        }
    }
    for f in &fails {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} failures", fails.len());
}

const MK: &str = "var log = []; function res(n) { return { [Symbol.dispose]() { log.push('d' + n); } }; } ";

fn with_mk(s: &str) -> String {
    format!("{MK}{s}")
}

/// Block, function, loop and generator scopes: dispose in reverse order at every exit.
#[test]
fn using_disposes_at_scope_exit() {
    let cases: Vec<(String, &str)> = vec![
        (with_mk("{ using a = res(1), b = res(2); using c = res(3); log.push('body'); } log.join()"), "body,d3,d2,d1"),
        (with_mk("{ using a = null, b = undefined; log.push('body'); } log.join()"), "body"),
        (with_mk("function f() { using a = res(1); log.push('b'); return 'r'; } var v = f(); v + ':' + log.join()"), "r:b,d1"),
        (with_mk("for (var i = 0; i < 3; i++) { using r = res(i); if (i == 1) continue; if (i == 2) break; log.push('b' + i); } log.join()"), "b0,d0,d1,d2"),
        (with_mk("for (using x of [res(1), res(2)]) log.push('b'); log.join()"), "b,d1,b,d2"),
        (with_mk("var i = 0; for (using r = res('h'); i < 2; i++) log.push('b' + i); log.join()"), "b0,b1,dh"),
        (with_mk("function* g() { using r = res('g'); yield 1; yield 2; } var it = g(); it.next(); it.return(); log.join()"), "dg"),
        (with_mk("outer: { using r = res(1); { using s = res(2); break outer; } } log.join()"), "d2,d1"),
        (with_mk("switch (1) { case 1: { using r = res(1); log.push('b'); } } log.join()"), "b,d1"),
        (with_mk("class C { static { using r = res('s'); log.push('b'); } } log.join()"), "b,ds"),
        (with_mk("try { using r = res(1); log.push('t'); } finally { log.push('f'); } log.join()"), "t,d1,f"),
        (with_mk("{ using s = new DisposableStack(); s.defer(() => log.push('x')); } log.join()"), "x"),
        (
            String::from("var log = []; var r = { get [Symbol.dispose]() { log.push('get'); return () => log.push('call'); } }; { using x = r; log.push('body'); } log.join()"),
            "get,body,call",
        ),
        (
            String::from("var seen; var r = { [Symbol.dispose]() { seen = this; } }; { using x = r; } String(seen === r)"),
            "true",
        ),
        // for-of bindings are fresh per iteration (closures see their own value)
        (String::from("var f = []; var o = [1, 2].map(() => ({ [Symbol.dispose]() {} })); for (using x of o) f.push(() => x); String(f[0]() === o[0] && f[1]() === o[1])"), "true"),
        // the outer bindings are untouched by the loop's own
        (String::from("var a = { [Symbol.dispose]() {} }, b = { [Symbol.dispose]() {} }, r; { using x = a; for (using x = b, i = null; !r; ) r = x === b; r = r && x === a; } String(r)"), "true"),
    ];
    let refs: Vec<(&str, &str)> = cases.iter().map(|(a, b)| (a.as_str(), *b)).collect();
    check(&refs);
}

/// AddDisposableResource's checks at the declaration, and DisposeResources' error folding.
#[test]
fn using_errors_and_suppression() {
    check(&[
        ("try { { using x = 1; } } catch (e) { e.name }", "TypeError"),
        ("try { { using x = {}; } } catch (e) { e.name }", "TypeError"),
        ("try { { using x = { [Symbol.dispose]: 1 }; } } catch (e) { e.name }", "TypeError"),
        ("try { { using x = { [Symbol.dispose]: null }; } } catch (e) { e.name }", "TypeError"),
        ("try { { using x = null; x = 1; } } catch (e) { e.name }", "TypeError"),
        ("try { for (using x of [null]) { x = 1; } } catch (e) { e.name }", "TypeError"),
        ("try { { x; using x = null; } } catch (e) { e.name }", "ReferenceError"),
        (
            "var e1 = new Error('1'); try { { using a = { [Symbol.dispose]() { throw e1; } }; } } catch (e) { String(e === e1) }",
            "true",
        ),
        (
            "var e1 = new Error('1'), e2 = new Error('2'), e3 = new Error('3'); var r; try { using a = { [Symbol.dispose]() { throw e1; } }; using b = { [Symbol.dispose]() { throw e2; } }; throw e3; } catch (e) { r = [e instanceof SuppressedError, e.error === e1, e.suppressed instanceof SuppressedError, e.suppressed.error === e2, e.suppressed.suppressed === e3].join(); } r",
            "true,true,true,true,true",
        ),
        (
            "var r; try { { using a = { [Symbol.dispose]() {} }; throw new Error('E'); } } catch (e) { r = e.message; } r",
            "E",
        ),
        (
            "var log = []; var r = { disposed: false, [Symbol.dispose]() { this.disposed = true; } }; function g() { throw new Error(); } try { var i = 0; for (using a = r, b = g(); i < 1; i++) {} } catch (e) { log.push(e.constructor.name); } log.join() + ',' + r.disposed",
            "Error,true",
        ),
        (
            "var r = { disposed: false, [Symbol.dispose]() { this.disposed = true; } }; try { { using a = r, b = 5; } } catch (e) { e.name + ',' + r.disposed }",
            "TypeError,true",
        ),
    ]);
}

/// `using` stays an identifier wherever the declaration grammar does not apply.
#[test]
fn using_as_identifier() {
    check(&[
        ("var using = [1, 2], x = 0; { using[x] = 5; } String(using)", "5,2"),
        ("var using, of = [[9], [8], [7]], r = []; for (using of of [0, 1, 2]) r.push(using); String(r)", "7"),
        ("{ using\nlet = 'v'; var using, let; } typeof let", "string"),
        ("var using = 3; { using instanceof Object; using in {}; } 'ok'", "ok"),
        ("function using() { return 'call'; } using()", "call"),
        ("String(eval('4; {using x = null;}'))", "4"),
        ("String(eval('{using a = null, b = null;}'))", "undefined"),
        ("class C { static { (() => { using await = null; }); } } 'ok'", "ok"),
        ("(function () { using f = function () {}; }); 'ok'", "ok"),
    ]);
}

/// The early errors: positions (Script top level, eval top level, case clauses, statement positions), no binding
/// patterns, an initializer on every binding, no for-in, the lexical-redeclaration rules.
#[test]
fn using_early_errors() {
    let bad = [
        "using x = null;",
        "await using x = null;",
        "(function () { eval('using x = null;'); })()",
        "{ using x; }",
        "{ using x = null, y; }",
        "{ using [] = null; }",
        "{ using x = null, [] = null; }",
        "{ using {} = null; }",
        "{ using x = null, {} = null; }",
        "switch (0) { case 0: using x = null; }",
        "switch (0) { default: using x = null; }",
        "for (using x in [1]) {}",
        "for (using x; ;) {}",
        "async function f() { for (await using x in [1]) {} }",
        "if (true) using x = null;",
        "label: using x = null;",
        "while (false) using x = null;",
        "{ using let = null; }",
        "(function () { 'use strict'; { using f = null; var f; } })",
        "{ using x = null; let x; }",
        "for (using x of []) { var x; }",
        "class C { static { using await = null; } }",
        "function f() { await using x = null; }",
        "async function f() { await using [] = null; }",
        "async function f() { await using x; }",
    ];
    let mut fails = Vec::new();
    for src in bad {
        let mut vm = Vm::new(Box::new(NullHost));
        let r = vm.run_script_str(src);
        let got = show(&mut vm, r);
        if !got.starts_with("THROW SyntaxError") {
            fails.push(format!("{src:?}: got {got:?}, want a SyntaxError"));
        }
    }
    let good = [
        "{ using x = null; } 'ok'",
        "function f() { using x = null; } 'ok'",
        "async function f() { await using x = null; for (await using y of []) {} for await (using z of []) {} } 'ok'",
        "async function* g() { await using x = null; } 'ok'",
        "async function f() { for (await using of of []) {} } 'ok'",
        "switch (0) { case 0: { using x = null; } } 'ok'",
        "for (using x = null; false; ) {} 'ok'",
        "try {} catch { using x = null; } 'ok'",
    ];
    for src in good {
        let mut vm = Vm::new(Box::new(NullHost));
        let r = vm.run_script_str(src);
        let got = show(&mut vm, r);
        if got != "ok" {
            fails.push(format!("{src:?}: got {got:?}, want ok"));
        }
    }
    for f in &fails {
        eprintln!("{f}");
    }
    assert!(fails.is_empty(), "{} failures", fails.len());
}

/// `await using`: async disposal awaited in reverse order, the @@dispose fallback, the implied Await only when a
/// declaration was evaluated, rejections folded into SuppressedError.
#[test]
fn await_using() {
    check_async(&[
        (
            "var out = []; (async function () { { await using a = { async [Symbol.asyncDispose]() { out.push('a'); } }, b = { [Symbol.dispose]() { out.push('b'); } }; out.push('body'); } out.push('after'); })();",
            "out.join()",
            "body,b,a,after",
        ),
        (
            "var log = []; var sync = true; async function f() { { await using x = null; log.push('in:' + sync); } log.push('out:' + sync); } f(); sync = false;",
            "log.join()",
            "in:true,out:false",
        ),
        (
            "var log = []; var sync = true; async function f() { outer: { if (true) break outer; await using x = null; } log.push('out:' + sync); } f(); sync = false;",
            "log.join()",
            "out:true",
        ),
        (
            "var out; (async function () { try { await using a = { async [Symbol.asyncDispose]() { throw 'A'; } }; throw 'B'; } catch (e) { out = [e.constructor.name, e.error, e.suppressed].join(); } })();",
            "out",
            "SuppressedError,A,B",
        ),
        (
            "var out; (async function () { try { await using a = { [Symbol.dispose]() { throw 'S'; } }; } catch (e) { out = e; } })();",
            "out",
            "S",
        ),
        (
            "var order = []; var r = { get [Symbol.asyncDispose]() { order.push('async'); return null; }, get [Symbol.dispose]() { order.push('sync'); return function () {}; } }; (async function () { { await using x = r; } order.push('done'); })();",
            "order.join()",
            "async,sync,done",
        ),
        (
            "var out = []; function mk(n) { return { async [Symbol.asyncDispose]() { out.push('d' + n); } }; } (async function () { for (await using x of [mk(1), mk(2)]) out.push('b'); out.push('end'); })();",
            "out.join()",
            "b,d1,b,d2,end",
        ),
        (
            "var out; (async function () { try { await using x = 1; } catch (e) { out = e.name; } })();",
            "out",
            "TypeError",
        ),
        (
            "var out; async function f() { await using x = { async [Symbol.asyncDispose]() { out = 'disposed'; } }; return 'r'; } f().then(v => out += ':' + v);",
            "out",
            "disposed:r",
        ),
        (
            "var out = []; async function* g() { await using x = { async [Symbol.asyncDispose]() { out.push('d'); } }; yield 1; out.push('after'); } (async function () { for await (var v of g()) out.push('v' + v); out.push('end'); })();",
            "out.join()",
            "v1,after,d,end",
        ),
    ]);
}

/// Module code: the top level may hold `using` and `await using`; disposal runs when the body completes.
#[test]
fn using_at_module_top_level() {
    let mut vm = Vm::new(Box::new(NullHost));
    let src = "globalThis.mlog = []; using a = { [Symbol.dispose]() { mlog.push('d'); } }; mlog.push('body');";
    if let Err(e) = vm.run_module("https://x.test/m.js", src) {
        panic!("module threw: {}", vm.error_string(&e));
    }
    let _ = vm.run_jobs();
    let r = vm.run_script_str("mlog.join()");
    assert_eq!(show(&mut vm, r), "body,d");

    let mut vm = Vm::new(Box::new(NullHost));
    let src = "globalThis.alog = []; await using a = { async [Symbol.asyncDispose]() { alog.push('d'); } }; alog.push('body');";
    if let Err(e) = vm.run_module("https://x.test/a.js", src) {
        panic!("module threw: {}", vm.error_string(&e));
    }
    let _ = vm.run_jobs();
    let r = vm.run_script_str("alog.join()");
    assert_eq!(show(&mut vm, r), "body,d");
}
