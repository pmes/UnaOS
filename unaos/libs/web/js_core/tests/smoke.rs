//! Known-answer checks of the engine on small programs (independent of test262).

use js_core::vm::{NullHost, Value, Vm};

fn eval(src: &str) -> String {
    let mut vm = Vm::new(Box::new(NullHost));
    match vm.run_script_str(src) {
        Ok(v) => {
            let _ = vm.run_jobs();
            match v {
                Value::String(s) => s.to_rust(),
                other => vm.error_string(&other),
            }
        }
        Err(e) => format!("THROW {}", vm.error_string(&e)),
    }
}

#[test]
fn smoke() {
    let cases: &[(&str, &str)] = &[
        ("1 + 2", "3"),
        ("'a' + 1", "a1"),
        ("var x = 10; function f(a) { return a * x; } f(4)", "40"),
        ("let s = 0; for (let i = 0; i < 10; i++) s += i; s", "45"),
        ("[1,2,3].map(x => x * 2).join()", "2,4,6"),
        ("JSON.stringify({a: [1, {b: 2}], c: 'x'})", "{\"a\":[1,{\"b\":2}],\"c\":\"x\"}"),
        ("class A { #x = 1; get x() { return this.#x; } } class B extends A { constructor() { super(); } } new B().x", "1"),
        ("function* g() { yield 1; yield 2; } [...g()].join('-')", "1-2"),
        ("let o = {a: 1, ...{b: 2}}; Object.keys(o).join()", "a,b"),
        ("try { null.x } catch (e) { e instanceof TypeError }", "true"),
        ("var r = []; for (var k in {a:1, b:2}) r.push(k); r.join()", "a,b"),
        ("(function() { return typeof arguments; })()", "object"),
        ("0.1 + 0.2", "0.30000000000000004"),
        ("(123.456).toFixed(2)", "123.46"),
        ("String(1e21)", "1e+21"),
        ("[3,1,2].sort().join()", "1,2,3"),
        ("Math.max(1, 5, 3)", "5"),
        ("let {a, b: [c, d = 4]} = {a: 1, b: [3]}; a + c + d", "8"),
        ("`x${1 + 1}y`", "x2y"),
        ("typeof Symbol.iterator", "symbol"),
        ("2n ** 64n", "18446744073709551616"),
        ("new Map([[1, 'a']]).get(1)", "a"),
        ("label: for (var i = 0; i < 3; i++) { for (;;) { continue label; } } i", "3"),
        ("var log = []; try { try { throw 1 } finally { log.push('f') } } catch (e) { log.push(e) } log.join()", "f,1"),
        ("(function f(n) { return n <= 1 ? 1 : n * f(n - 1) })(10)", "3628800"),
        ("var a = []; a[5] = 1; a.length", "6"),
        ("eval('var q = 5; q * 2')", "10"),
        ("var p = Promise.resolve(1); var out; p.then(v => out = v); out", "undefined"),
    ];
    let mut fails = Vec::new();
    for (src, want) in cases {
        let got = eval(src);
        if got != *want {
            fails.push(format!("{:?}: got {:?}, want {:?}", src, got, want));
        }
    }
    for f in &fails {
        eprintln!("{}", f);
    }
    assert!(fails.is_empty(), "{} smoke failures", fails.len());
}
