//! AETHERPERF (LEDGER SR118): ten interpreter micro-benchmarks, ns per operation, for the before/after of the
//! fast paths (hidden classes, inline caches, ropes, packed-array element paths, the small-integer arithmetic).
//!
//! Not part of the default test run (`#[ignore]`). The seat runs, at the base and at the tip:
//!     cargo test --release -p js_core --test perf -- --ignored --nocapture
//! Each kernel defines `bench(n)` doing `n` operations and returning a checksum; it is warmed once (inline caches
//! and shapes filled), then timed. One line per kernel:
//!     PERF name=<kernel> ns_per_op=<f> n=<n> check=<checksum>
//! The `check=` values must be identical before and after (the fast paths change speed, never results).
//! `JS_CORE_PERF_SCALE=<f>` scales every `n` (default 1.0).

use js_core::vm::{NullHost, Value, Vm};
use std::time::Instant;

struct Kernel {
    name: &'static str,
    n: u32,
    src: &'static str,
}

const KERNELS: &[Kernel] = &[
    // 1. Named property access: monomorphic loads and stores on one object shape (GetOwn / SetOwn hits).
    Kernel {
        name: "prop_access",
        n: 400_000,
        src: "function bench(n) { var o = {x: 1, y: 2, z: 3, w: 4}; var s = 0;
                for (var i = 0; i < n; i++) { o.x = i; s += o.x + o.y + o.z + o.w; o.w = s & 1023; } return s; }",
    },
    // 2. Method call: a class method found on the prototype (GetProto hit) plus `this.x` stores in a constructor
    //    (SetAdd hits through one transition chain).
    Kernel {
        name: "method_call",
        n: 200_000,
        src: "class P { constructor(x, y) { this.x = x; this.y = y; } len2() { return this.x * this.x + this.y * this.y; }
                      add(o) { return new P(this.x + o.x, this.y + o.y); } }
              function bench(n) { var a = new P(1, 2), b = new P(0, 1), s = 0;
                for (var i = 0; i < n; i++) { a = a.add(b); s = (s + a.len2()) % 1000003; } return s; }",
    },
    // 3. Array push: `push` through Array.prototype (GetProto + native call) and `a[a.length] = v` appends.
    Kernel {
        name: "array_push",
        n: 400_000,
        src: "function bench(n) { var s = 0; var a = [];
                for (var i = 0; i < n; i++) { if ((i & 1023) === 0) { s += a.length; a = []; } a.push(i); a[a.length] = i & 7; }
                return s + a.length; }",
    },
    // 4. Array index: dense packed element loads / stores and small-integer arithmetic (% | & >>).
    Kernel {
        name: "array_index",
        n: 400_000,
        src: "var A = []; for (var i = 0; i < 1024; i++) A.push(i * 3 % 17);
              function bench(n) { var s = 0;
                for (var i = 0; i < n; i++) { var j = i & 1023; var v = A[j]; A[j] = (v + i) % 31; s = (s + (v | 0) * 7 + (j >> 2)) % 65521; }
                return s; }",
    },
    // 5. String concatenation: `+=` building (ropes), then one read that flattens.
    Kernel {
        name: "string_concat",
        n: 200_000,
        src: "function bench(n) { var total = 0; var s = '';
                for (var i = 0; i < n; i++) { s += 'item-' + (i & 255) + ';'; if ((i & 4095) === 4095) { total += s.length + s.charCodeAt(s.length >> 1); s = ''; } }
                return total + s.length; }",
    },
    // 6. Closure call: calls of a captured-environment function value.
    Kernel {
        name: "closure_call",
        n: 400_000,
        src: "function mk(k) { var c = 0; return function (x) { c = (c + x * k) % 9973; return c; }; }
              function bench(n) { var f = mk(3), g = mk(5), s = 0;
                for (var i = 0; i < n; i++) s = (s + f(i) + g(s & 255)) % 100003; return s; }",
    },
    // 7. JSON.parse of a small document (n = documents parsed).
    Kernel {
        name: "json_parse",
        n: 5_000,
        src: "var DOC = JSON.stringify({id: 7, name: 'widget', tags: ['a', 'b', 'c'], dims: {w: 3, h: 4, d: null}, price: 12.5, ok: true,
                                       items: [{k: 1, v: 'one'}, {k: 2, v: 'two'}, {k: 3, v: 'three'}]});
              function bench(n) { var s = 0; for (var i = 0; i < n; i++) { var o = JSON.parse(DOC); s += o.items.length + o.dims.h + o.tags[2].length; } return s; }",
    },
    // 8. JSON.stringify of a small object graph (n = documents serialized).
    Kernel {
        name: "json_stringify",
        n: 5_000,
        src: "var OBJ = {id: 7, name: 'widget', tags: ['a', 'b', 'c'], dims: {w: 3, h: 4, d: null}, price: 12.5, ok: true,
                         items: [{k: 1, v: 'one'}, {k: 2, v: 'two'}, {k: 3, v: 'three'}]};
              function bench(n) { var s = 0; for (var i = 0; i < n; i++) { OBJ.id = i; s += JSON.stringify(OBJ).length; } return s; }",
    },
    // 9. RegExp exec: a global regexp scanning a line with captures (n = exec calls).
    Kernel {
        name: "regexp_exec",
        n: 50_000,
        src: "var LINE = 'GET /a/b/c?x=1 200 512; POST /login 302 0; GET /img/logo.png 304 0; GET /api/v1/items 200 2048; ';
              function bench(n) { var re = /(GET|POST) (\\S+) (\\d{3}) (\\d+);/g, s = 0, m;
                for (var i = 0; i < n; i++) { m = re.exec(LINE); if (m === null) { m = re.exec(LINE); } s += m[2].length + (+m[4] & 255); }
                return s; }",
    },
    // 10. A DOM-free "todo app": a model of records with add / toggle / filter / render-to-string / remove.
    Kernel {
        name: "todo_app",
        n: 2_000,
        src: "class Todo { constructor(id, title) { this.id = id; this.title = title; this.done = false; } toggle() { this.done = !this.done; } }
              class Store { constructor() { this.items = []; this.next = 1; this.listeners = []; }
                on(f) { this.listeners.push(f); } emit() { for (var f of this.listeners) f(this); }
                add(t) { this.items.push(new Todo(this.next++, t)); this.emit(); }
                toggle(id) { var t = this.items.find(x => x.id === id); if (t) t.toggle(); this.emit(); }
                clear() { this.items = this.items.filter(x => !x.done); this.emit(); }
                left() { return this.items.reduce((s, x) => s + (x.done ? 0 : 1), 0); } }
              function render(st) { var out = '<ul>'; for (var t of st.items) out += '<li class=\"' + (t.done ? 'done' : 'todo') + '\">' + t.title + '</li>';
                return out + '</ul><p>' + st.left() + ' left</p>'; }
              function bench(n) { var total = 0;
                for (var r = 0; r < n; r++) { var st = new Store(); var html = ''; st.on(s => { html = render(s); });
                  for (var i = 0; i < 8; i++) st.add('task ' + i + ' of round ' + (r & 15));
                  st.toggle(2); st.toggle(5); st.toggle(7); st.clear();
                  total += html.length + st.left() + JSON.stringify(st.items.map(t => ({id: t.id, done: t.done}))).length; }
                return total; }",
    },
];

fn value_string(vm: &mut Vm, v: Result<Value, Value>) -> String {
    match v {
        Ok(Value::String(s)) => s.to_rust(),
        Ok(other) => vm.error_string(&other),
        Err(e) => format!("THROW {}", vm.error_string(&e)),
    }
}

#[test]
#[ignore]
fn perf() {
    let scale: f64 = std::env::var("JS_CORE_PERF_SCALE").ok().and_then(|s| s.parse().ok()).unwrap_or(1.0);
    for k in KERNELS {
        let n = ((k.n as f64) * scale).max(1.0) as u32;
        let mut vm = Vm::new(Box::new(NullHost));
        let setup = vm.run_script_str(k.src);
        if let Err(e) = setup {
            let msg = vm.error_string(&e);
            println!("PERF name={} ns_per_op=- n={} check=SETUP-THROW {}", k.name, n, msg);
            continue;
        }
        // Warm: fill shapes and inline caches.
        let warm = (n / 10).max(1);
        let w = vm.run_script_str(&format!("String(bench({}))", warm));
        let _ = value_string(&mut vm, w);
        let t = Instant::now();
        let r = vm.run_script_str(&format!("String(bench({}))", n));
        let dt = t.elapsed();
        let check = value_string(&mut vm, r);
        let ns = dt.as_nanos() as f64 / n as f64;
        println!("PERF name={} ns_per_op={:.1} n={} check={}", k.name, ns, n, check);
    }
}
