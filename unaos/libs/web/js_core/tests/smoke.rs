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

/// M3 built-ins: RegExp, typed arrays, Date, iterator helpers, Error stack.
#[test]
fn builtins_kat() {
    let cases: &[(&str, &str)] = &[
        ("'2024-01-15'.replace(/(\\d+)-(\\d+)-(\\d+)/, '$3/$2/$1')", "15/01/2024"),
        ("/(?<y>\\d{4})/.exec('in 1999 ok').groups.y", "1999"),
        ("/(?<=\\$)\\d+/.exec('cost $42')[0]", "42"),
        ("/^\\p{Lu}+$/u.test('ÄÖÜ')", "true"),
        ("/[\\p{Script=Greek}--[α]]/v.test('β') + ',' + /[\\p{Script=Greek}--[α]]/v.test('α')", "true,false"),
        ("/(?i:a)b/.test('Ab') + ',' + /(?i:a)b/.test('AB')", "true,false"),
        ("JSON.stringify('aBc'.match(/b/gi))", "[\"B\"]"),
        ("'a1b2c3'.split(/\\d/).join('|')", "a|b|c|"),
        ("/(a+)+b/.test('aaaaaaaaaaaaaaaaaaaaaab')", "true"),
        ("/x/dg.flags + /a/v.unicodeSets", "dgtrue"),
        ("[...'abcabc'.matchAll(/b(c)/g)].map(m => m.index).join()", "1,4"),
        ("RegExp.escape('a.b*c')", "\\x61\\.b\\*c"),
        ("var t = new Float32Array([1.5, -2]); var u = new Uint8Array(t.buffer); u.length + ':' + t[1]", "8:-2"),
        ("new Uint8ClampedArray([300, -5, 1.5, 2.5]).join()", "255,0,2,2"),
        ("var dv = new DataView(new ArrayBuffer(8)); dv.setUint16(0, 0x4142); dv.getUint8(0) + ',' + dv.getUint16(0, true)", "65,16961"),
        ("var b = new ArrayBuffer(4, {maxByteLength: 8}); var a = new Uint8Array(b); b.resize(8); a.length", "8"),
        ("new BigInt64Array([-1n])[0] + ''", "-1"),
        ("new Float16Array([1.0009765625, 65520])[0] + ',' + new Float16Array([65520])[0]", "1.0009765625,Infinity"),
        ("Atomics.add(new Int32Array(new SharedArrayBuffer(8)), 0, 5)", "0"),
        ("new Date(Date.UTC(2020, 1, 29, 23, 59, 59, 999)).toISOString()", "2020-02-29T23:59:59.999Z"),
        ("Date.parse('2000-01-01T00:00:00Z')", "946684800000"),
        ("new Date(8.64e15 + 1).getTime()", "NaN"),
        ("var d = new Date(0); d.setUTCMonth(13); d.toISOString()", "1971-02-01T00:00:00.000Z"),
        ("new Date(0).toUTCString()", "Thu, 01 Jan 1970 00:00:00 GMT"),
        ("[1,2,3,4,5].values().chunks(2).toArray().join('|')", "1,2|3,4|5"),
        ("[1,2,3,4].values().windows(3).toArray().join('|')", "1,2,3|2,3,4"),
        ("[1,2,3].values().map(x => x * 2).filter(x => x > 2).toArray().join()", "4,6"),
        ("typeof new Error('x').stack", "string"),
        ("Math.sqrt(2) + ',' + Math.hypot(3, 4) + ',' + Math.sinh(1)", "1.4142135623730951,5,1.1752011936438014"),
        ("(1e-21).toPrecision(16)", "9.999999999999999e-22"),
        ("10 - 2 * 3 + 2 ** 3 ** 2 / 64", "12"),
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
    assert!(fails.is_empty(), "{} built-in KAT failures", fails.len());
}

/// Time zones as data: POSIX TZ rules drive LocalTZA (US Eastern and Central European rules, both sides of
/// the transitions).
#[test]
fn tz_rules() {
    use js_core::tz::PosixTz;
    let ny = PosixTz::parse("EST5EDT,M3.2.0,M11.1.0").unwrap();
    // 2024-03-10 06:59:59Z is 01:59:59 EST; 07:00:00Z is 03:00:00 EDT.
    assert_eq!(ny.offset_at_utc(1710053999000.0), -5.0 * 3600e3);
    assert_eq!(ny.offset_at_utc(1710054000000.0), -4.0 * 3600e3);
    // 2024-11-03 05:59:59Z is 01:59:59 EDT; 06:00:00Z is 01:00:00 EST.
    assert_eq!(ny.offset_at_utc(1730613599000.0), -4.0 * 3600e3);
    assert_eq!(ny.offset_at_utc(1730613600000.0), -5.0 * 3600e3);
    let cet = PosixTz::parse("CET-1CEST,M3.5.0,M10.5.0/3").unwrap();
    // 2024-03-31 01:00:00Z: CEST starts; 2024-10-27 01:00:00Z: CET again.
    assert_eq!(cet.offset_at_utc(1711846799000.0), 3600e3);
    assert_eq!(cet.offset_at_utc(1711846800000.0), 7200e3);
    assert_eq!(cet.offset_at_utc(1729990800000.0), 3600e3);
    assert_eq!(PosixTz::parse("<+0530>-5:30").unwrap().std_offset, 5.5 * 3600e3);
    // Engine integration: local time follows the rule.
    let mut vm = Vm::new(Box::new(NullHost));
    vm.tz = Some(ny);
    let v = vm.run_script_str("var d = new Date(Date.UTC(2024, 6, 4, 16)); d.getHours() + ' ' + d.getTimezoneOffset() + ' ' + d.toString()").unwrap();
    match v {
        Value::String(s) => assert_eq!(s.to_rust(), "12 240 Thu Jul 04 2024 12:00:00 GMT-0400 (EDT)"),
        _ => panic!("not a string"),
    }
}

/// Host hooks: timers on the virtual-time event loop interleave with microtasks in HTML order.
#[test]
fn event_loop_order() {
    use std::cell::RefCell;
    use std::rc::Rc;
    struct H(Rc<RefCell<Vec<String>>>);
    impl js_core::vm::Host for H {
        fn console(&mut self, _l: u8, m: &str) {
            self.0.borrow_mut().push(m.to_string());
        }
    }
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut vm = Vm::new(Box::new(H(log.clone())));
    js_core::builtins::host::install_host_globals(&mut vm);
    vm.run_script_str("setTimeout(() => console.log('t20'), 20); setTimeout(() => { console.log('t0'); Promise.resolve().then(() => console.log('m-in-t0')); }, 0); var n = 0, h = setInterval(() => { console.log('i' + ++n); if (n == 2) clearInterval(h); }, 8); queueMicrotask(() => console.log('qm')); console.log('sync');").unwrap();
    vm.run_event_loop(100).unwrap();
    assert_eq!(log.borrow().join(" "), "sync qm t0 m-in-t0 i1 i2 t20");
}

/// Exact GC: programs survive a collection at every safepoint (natives keep values they receive rooted).
#[test]
fn gc_stress() {
    let progs = [
        "function f(x) { return Math.max(x, 0); } [1, 2, 3].map(f).concat([4]).join()",
        "var fs = [x => x + 1]; [1, 2].map(x => fs.reduceRight((a, g) => g(a), x)).join()",
        "var out = []; Promise.resolve(1).then(v => out.push(v)); Promise.all([2, Promise.resolve(3)]).then(v => out.push(v.join('+'))); 'ok'",
        "function* g() { var s = []; for (var i = 0; i < 20; i++) { s.push({ i }); yield s.length; } } [...g()].length",
        "var m = new Map(); for (var i = 0; i < 50; i++) m.set({ i }, [i]); var w = new WeakMap([[m, 1]]); m.size",
        "JSON.stringify(JSON.parse('[1,{\"a\":[2,3]}]', (k, v) => v), null, 0)",
        "'a-b-c'.replace(/-/g, () => '+'.repeat(2)).split('+').filter(Boolean).join()",
    ];
    let want = ["1,2,3,4", "2,3", "ok", "20", "50", "[1,{\"a\":[2,3]}]", "a,b,c"];
    for (p, w) in progs.iter().zip(want.iter()) {
        let mut vm = Vm::new(Box::new(NullHost));
        vm.gc_stress = true;
        let v = vm.run_script_str(p).unwrap();
        vm.run_jobs().unwrap();
        assert_eq!(vm.error_string(&v), *w, "{}", p);
    }
}
