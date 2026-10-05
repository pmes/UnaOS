//! `js_core-fuzz`: deterministic mutation fuzzing of the whole engine (parser, compiler, interpreter, built-ins).
//!
//! cargo run --release -p js_core --example js_core-fuzz -- [-n 10000] [--seed S] [--corpus <test262>] [--mem-mb 512]
//!
//! Seeds come from the oracle pages, built-in snippets and (optionally) a sample of test262 files. Each mutant runs
//! in a child process (`--child`) under an address-space cap (`ulimit -v`), an instruction budget, a heap-cell
//! cap, a string-length cap and a wall-clock timeout. A panic, abort or signal in a child is an engine failure;
//! JavaScript exceptions and resource terminations are expected outcomes. Every eighth mutant runs with the
//! collector at every safepoint (`gc_stress`), which turns a missing GC root into a deterministic crash.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        // xorshift64*
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: usize) -> usize {
        if n == 0 { 0 } else { (self.next() % n as u64) as usize }
    }
}

const STUB_HARNESS: &str = r#"
function Test262Error(m) { this.message = m || ""; }
function $DONOTEVALUATE() {}
var assert = function (c, m) { if (c !== true) throw new Test262Error(m); };
assert.sameValue = function (a, b, m) { if (!Object.is(a, b)) throw new Test262Error(m); };
assert.notSameValue = function (a, b, m) { if (Object.is(a, b)) throw new Test262Error(m); };
assert.throws = function (C, f, m) { try { f(); } catch (e) { return; } throw new Test262Error(m); };
assert.compareArray = function (a, b, m) { if (String(a) !== String(b)) throw new Test262Error(m); };
var compareArray = function (a, b) { return String(a) === String(b); };
function verifyProperty() {}
var $262 = { createRealm: function () { return $262; }, evalScript: function (s) { return (0, eval)(s); }, gc: function () {}, global: globalThis, detachArrayBuffer: function () {} };
function print() {}
"#;

const SNIPPETS: &[&str] = &[
    "0", "-0", "1e309", "-1", "2**31", "2**32-1", "2**53", "NaN", "Infinity", "-Infinity", "0.1", "1n", "-(2n**64n)",
    "''", "'\\uD800'", "'a'.repeat(1000)", "[]", "[1,,3]", "{}", "null", "undefined", "Symbol()", "Symbol.iterator",
    "new Proxy({}, {})", "new Proxy(function(){}, { apply() { throw 1; } })", "Object.create(null)", "this",
    "function(){}", "() => {}", "async () => { await 0; }", "function*(){ yield* [1,2]; }", "class extends Array {}",
    "new Uint8Array(8)", "new ArrayBuffer(8, { maxByteLength: 16 })", "new DataView(new ArrayBuffer(4))",
    "/(a+)+b/", "/(?<n>x)|\\k<n>/u", "new Date(NaN)", "new Map([[1,2]])", "new WeakRef({})", "Promise.reject(1)",
    "Object.freeze([])", "arguments", "new.target", "super.x", "eval('1')", "globalThis", "Array(2**32-1)",
    "Reflect.ownKeys(globalThis)", "JSON.parse('[[[[1]]]]')", "Math.max", "String.prototype", "Array.prototype",
];

const STATEMENTS: &[&str] = &[
    "Object.prototype[0] = 1;", "Array.prototype.length = 0;", "delete Array.prototype[Symbol.iterator];",
    "Object.defineProperty(Object.prototype, 'then', { get() { return function(r){ r(1); }; } });",
    "Symbol.prototype.toString = null;", "for (var __i = 0; __i < 100; __i++) {}", "try { throw 1 } catch {}",
    "label: { break label; }", "with ({}) { }", "debugger;", "var __a = []; __a[100] = 1; __a.sort();",
    "Object.setPrototypeOf(Array.prototype, null);", "String.prototype.split = function () { return []; };",
    "$262.detachArrayBuffer(new ArrayBuffer(1));", "(function f(){ f(); })();", "new Function('a', 'return a')(1);",
    "async function __af() { for await (const x of [1]) {} } __af();", "Promise.resolve().then(() => { throw 0; });",
    "setTimeout(() => {}, 0);", "gc && 0;",
];

fn builtin_seeds() -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for p in ["page1.html", "page3.html"] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/oracle").join(p);
        if let Ok(s) = std::fs::read_to_string(path) {
            if let (Some(a), Some(b)) = (s.find("<script>"), s.find("</script>")) {
                let body = s[a + 8..b].replace("document.getElementById(\"out\").textContent", "var __out").replace("document.", "({ getElementById(){ return { appendChild(){} } }, createElement(){ return {} } }).");
                v.push(body);
            }
        }
    }
    v.push("var o = { a: 1, get b() { return this.a; } }; for (var k in o) o[k]; JSON.stringify(o, null, 2);".into());
    v.push("class P { #x = 1; static m(o) { return #x in o; } } P.m(new P()); [1,2,3].map(x => x * 2).reduce((a, b) => a + b);".into());
    v.push("function* g() { try { yield 1; } finally { yield 2; } } var it = g(); it.next(); it.return(5); [...g()];".into());
    v.push("var s = new Set([1,2]); var m = new Map(); m.set(s, [s]); var w = new WeakMap([[s, 1]]); s.union(new Set([3])).isSubsetOf(s);".into());
    v.push("'abc'.replace(/(?<x>b)/g, '[$<x>]').split('').toReversed().join(); /\\p{L}+/v.exec('héllo');".into());
    v.push("var ta = new Float64Array([3,1,2]); ta.sort(); ta.subarray(1).set([9]); new BigInt64Array(ta.buffer)[0];".into());
    v.push("async function f() { await null; throw new Error('x'); } f().catch(e => e.message); Promise.all([1, Promise.resolve(2)]);".into());
    v.push("var d = new Date(2020, 1, 29); d.setMonth(12); d.toISOString(); Date.parse(d.toString());".into());
    v.push("label: for (var i of [1,2,3]) { for (var j in {a:1}) { if (i > 1) break label; continue label; } }".into());
    v.push("var {a, ...r} = {a:1, b:2}; var [x = 1, ...y] = 'str'; ({ [Symbol.toPrimitive]() { return 1; } }) + 1;".into());
    v
}

fn test262_seeds(root: &PathBuf, every: usize) -> Vec<String> {
    let mut files = Vec::new();
    let mut stack = vec![root.join("test/language"), root.join("test/built-ins")];
    while let Some(d) = stack.pop() {
        if let Ok(rd) = std::fs::read_dir(&d) {
            let mut es: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
            es.sort();
            for p in es {
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().map(|e| e == "js").unwrap_or(false) && !p.to_string_lossy().contains("_FIXTURE") {
                    files.push(p);
                }
            }
        }
    }
    files.sort();
    files.iter().step_by(every.max(1)).filter_map(|p| std::fs::read_to_string(p).ok()).filter(|s| !s.contains("flags: [module]") && s.len() < 20_000).collect()
}

fn mutate(rng: &mut Rng, seeds: &[String]) -> String {
    let base = &seeds[rng.below(seeds.len())];
    let mut s: Vec<char> = base.chars().collect();
    let rounds = 1 + rng.below(4);
    for _ in 0..rounds {
        if s.is_empty() {
            s = "1".chars().collect();
        }
        let n = s.len();
        match rng.below(8) {
            0 => {
                // replace a number-ish run with an interesting value
                let i = rng.below(n);
                if let Some(start) = (i..n).find(|&k| s[k].is_ascii_digit()) {
                    let mut end = start;
                    while end < n && (s[end].is_ascii_alphanumeric() || s[end] == '.') {
                        end += 1;
                    }
                    let rep: Vec<char> = SNIPPETS[rng.below(9)].chars().collect();
                    s.splice(start..end, rep);
                }
            }
            1 => {
                // insert a snippet expression at a random position
                let i = rng.below(n + 1);
                let rep: Vec<char> = SNIPPETS[rng.below(SNIPPETS.len())].chars().collect();
                s.splice(i..i, rep);
            }
            2 => {
                // delete a random span
                let i = rng.below(n);
                let l = 1 + rng.below(16.min(n - i));
                s.drain(i..i + l);
            }
            3 => {
                // duplicate a random span
                let i = rng.below(n);
                let l = 1 + rng.below(64.min(n - i));
                let dup: Vec<char> = s[i..i + l].to_vec();
                let at = rng.below(s.len() + 1);
                s.splice(at..at, dup);
            }
            4 => {
                // splice a line from another seed
                let other = &seeds[rng.below(seeds.len())];
                let lines: Vec<&str> = other.lines().collect();
                if !lines.is_empty() {
                    let line: Vec<char> = lines[rng.below(lines.len())].chars().collect();
                    let at = rng.below(s.len() + 1);
                    let mut ins = vec!['\n'];
                    ins.extend(line);
                    ins.push('\n');
                    s.splice(at..at, ins);
                }
            }
            5 => {
                // prepend a hostile statement
                let st: Vec<char> = STATEMENTS[rng.below(STATEMENTS.len())].chars().collect();
                s.splice(0..0, st);
            }
            6 => {
                // flip a punctuator
                let i = rng.below(n);
                let p = ['(', ')', '[', ']', '{', '}', ',', ';', '.', '=', '+', '*', '?', ':', '!', '`', '"', '\'', '/', '\\'];
                s[i] = p[rng.below(p.len())];
            }
            _ => {
                // swap two spans
                let a = rng.below(n);
                let b = rng.below(n);
                let (a, b) = if a < b { (a, b) } else { (b, a) };
                let la = 1 + rng.below(8.min(b - a + 1));
                let lb = 1 + rng.below(8.min(n - b));
                if a + la <= b {
                    let x: Vec<char> = s[a..a + la].to_vec();
                    let y: Vec<char> = s[b..b + lb].to_vec();
                    s.splice(b..b + lb, x);
                    s.splice(a..a + la, y);
                }
            }
        }
    }
    s.into_iter().collect()
}

fn peak_rss_kb() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).and_then(|l| l.split_whitespace().nth(1)).and_then(|v| v.parse().ok()))
        .unwrap_or(0)
}

fn child(stress: bool) -> i32 {
    use js_core::vm::*;
    let mut src = String::new();
    std::io::stdin().read_to_string(&mut src).unwrap();
    let r = std::panic::catch_unwind(move || {
        let mut vm = Vm::new(Box::new(NullHost));
        js_core::builtins::host::install_host_globals(&mut vm);
        vm.budget = Some(if stress { 1_000_000 } else { 5_000_000 });
        vm.gc_stress = stress;
        vm.max_cells = 300_000;
        vm.max_string_len = 1 << 22;
        let full = format!("{}\n{}", STUB_HARNESS, src);
        let units: Vec<u16> = full.encode_utf16().collect();
        let outcome = match vm.run_script(&units).and_then(|_| vm.run_event_loop(1000)) {
            Ok(()) => 'o',
            Err(_) if vm.out_of_memory => 'm',
            Err(_) if vm.terminated => 'b',
            Err(_) => 'e',
        };
        outcome
    });
    match r {
        Ok(c) => {
            println!("{} {}", c, peak_rss_kb());
            0
        }
        Err(_) => 101,
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|a| a == "--child").unwrap_or(false) {
        std::process::exit(child(false));
    }
    if args.first().map(|a| a == "--child-stress").unwrap_or(false) {
        std::process::exit(child(true));
    }
    let mut n = 10_000usize;
    let mut seed = 0x5EED_1234_ABCD_0001u64;
    let mut corpus: Option<PathBuf> = None;
    let mut mem_mb = 512u64;
    let mut save: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-n" => n = args[i + 1].parse().unwrap(),
            "--seed" => seed = args[i + 1].parse().unwrap(),
            "--corpus" => corpus = Some(PathBuf::from(&args[i + 1])),
            "--mem-mb" => mem_mb = args[i + 1].parse().unwrap(),
            "--save" => save = Some(PathBuf::from(&args[i + 1])),
            _ => {
                i += 1;
                continue;
            }
        }
        i += 2;
    }
    let mut seeds = builtin_seeds();
    if let Some(root) = &corpus {
        seeds.extend(test262_seeds(root, 40));
    }
    let exe = std::env::current_exe().unwrap();
    let mut rng = Rng(seed);
    let (mut ok, mut err, mut budget, mut oom, mut timeout, mut crash) = (0, 0, 0, 0, 0, 0);
    let mut max_rss = 0u64;
    let start = Instant::now();
    for k in 0..n {
        let m = mutate(&mut rng, &seeds);
        let mut c = Command::new("sh")
            .arg("-c")
            .arg(format!("ulimit -v {}; exec \"$0\" {}", mem_mb * 1024, if k % 8 == 7 { "--child-stress" } else { "--child" }))
            .arg(&exe)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn");
        c.stdin.take().unwrap().write_all(m.as_bytes()).unwrap();
        let t0 = Instant::now();
        let status = loop {
            if let Some(st) = c.try_wait().unwrap() {
                break Some(st);
            }
            if t0.elapsed() > Duration::from_secs(10) {
                let _ = c.kill();
                let _ = c.wait();
                break None;
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        let mut out = String::new();
        let mut errout = String::new();
        let _ = c.stdout.take().unwrap().read_to_string(&mut out);
        let _ = c.stderr.take().unwrap().read_to_string(&mut errout);
        match status {
            None => {
                timeout += 1;
                if let Some(dir) = &save {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(dir.join(format!("timeout-{}.js", k)), &m);
                }
            }
            Some(st) if st.success() => {
                let mut parts = out.split_whitespace();
                match parts.next() {
                    Some("o") => ok += 1,
                    Some("e") => err += 1,
                    Some("b") => budget += 1,
                    Some("m") => oom += 1,
                    _ => {}
                }
                if let Some(r) = parts.next().and_then(|v| v.parse::<u64>().ok()) {
                    max_rss = max_rss.max(r);
                }
            }
            Some(_) => {
                crash += 1;
                eprintln!("CRASH mutant {}: {}", k, errout.lines().find(|l| l.contains("panicked") || l.contains("fatal") || l.contains("memory")).unwrap_or("(no message)"));
                if let Some(dir) = &save {
                    let _ = std::fs::create_dir_all(dir);
                    let _ = std::fs::write(dir.join(format!("crash-{}.js", k)), &m);
                }
            }
        }
    }
    println!(
        "fuzz: {} mutants in {:.1}s (seed {:#x}, {} seeds): completed {}, threw {}, budget-terminated {}, heap-capped {}, timeouts {}, crashes {}; peak child RSS {} KiB (cap {} MiB)",
        n,
        start.elapsed().as_secs_f64(),
        seed,
        seeds.len(),
        ok,
        err,
        budget,
        oom,
        timeout,
        crash,
        max_rss,
        mem_mb
    );
    if crash > 0 {
        std::process::exit(1);
    }
}
