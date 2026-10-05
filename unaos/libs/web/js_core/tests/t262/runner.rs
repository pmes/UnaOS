//! The test262 driver shared by the `js_core-test262` example and the `cargo test` gate.

use super::host::{install, TestHost};
use super::meta::{parse_meta, Meta, OUT_OF_SCOPE};
use js_core::vm::{Value, Vm};
use std::cell::RefCell;
use std::rc::Rc;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}

pub struct Config {
    pub root: PathBuf,
    pub parse_only: bool,
    pub filters: Vec<String>,
    pub verbose: bool,
    pub json: Option<PathBuf>,
    pub threads: usize,
}

pub fn collect(root: &Path, filters: &[String]) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let test = root.join("test");
    let mut stack = vec![test.clone()];
    while let Some(d) = stack.pop() {
        let rd = match std::fs::read_dir(&d) {
            Ok(r) => r,
            Err(_) => continue,
        };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                let name = p.file_name().unwrap().to_string_lossy().to_string();
                if d == test && (name == "intl402" || name == "staging") {
                    continue;
                }
                stack.push(p);
            } else if p.extension().map(|x| x == "js").unwrap_or(false) {
                let s = p.to_string_lossy();
                if s.contains("_FIXTURE") {
                    continue;
                }
                let rel = p.strip_prefix(&test).unwrap().to_string_lossy().to_string();
                if filters.is_empty() || filters.iter().any(|f| rel.contains(f.as_str())) {
                    out.push(p);
                }
            }
        }
    }
    out.sort();
    out
}

fn utf16(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

/// Parse-only check of one test. Returns (outcome, detail).
pub fn run_parse(src: &str, m: &Meta) -> (Outcome, String) {
    let negative_parse = m.negative_phase.as_deref() == Some("parse");
    let module = m.has_flag("module");
    let mut variants: Vec<bool> = Vec::new(); // strict?
    if module || m.has_flag("raw") {
        variants.push(false);
    } else {
        if !m.has_flag("onlyStrict") {
            variants.push(false);
        }
        if !m.has_flag("noStrict") {
            variants.push(true);
        }
    }
    for strict in variants {
        let text = if strict { format!("\"use strict\";\n{}", src) } else { src.to_string() };
        let u = utf16(&text);
        let r = if module { js_core::parser::parse_module(&u) } else { js_core::parser::parse_script(&u) };
        match (r, negative_parse) {
            (Ok(_), true) => return (Outcome::Fail, format!("expected SyntaxError (strict={})", strict)),
            (Err(e), false) => {
                let upto: String = char::decode_utf16(u[..(e.pos as usize).min(u.len())].iter().copied()).map(|c| c.unwrap_or('?')).collect();
                let line = upto.matches('\n').count() + 1;
                return (Outcome::Fail, format!("unexpected SyntaxError (strict={}) line {}: {}", strict, line, e.msg));
            }
            _ => {}
        }
    }
    (Outcome::Pass, String::new())
}

/// Harness file cache.
pub struct Harness {
    pub dir: PathBuf,
    pub files: std::collections::HashMap<String, String>,
}

impl Harness {
    pub fn load(root: &Path) -> Harness {
        let dir = root.join("harness");
        let mut files = std::collections::HashMap::new();
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.extension().map(|x| x == "js").unwrap_or(false) {
                    if let Ok(s) = std::fs::read_to_string(&p) {
                        files.insert(p.file_name().unwrap().to_string_lossy().to_string(), s);
                    }
                }
            }
        }
        Harness { dir, files }
    }
}

/// Run one test in one mode (strict or sloppy). Returns Ok(()) or a failure description.
pub fn run_one(h: &Harness, path: &Path, src: &str, m: &Meta, strict: bool) -> Result<(), String> {
    let out = Rc::new(RefCell::new(String::new()));
    let host = TestHost { out: out.clone(), base: path.parent().unwrap().to_path_buf() };
    let mut vm = Vm::new(Box::new(host));
    vm.budget = Some(400_000_000);
    install(&mut vm);
    let raw = m.has_flag("raw");
    let module = m.has_flag("module");
    let is_async = m.has_flag("async");
    let negative_type = m.negative_type.clone();
    let negative_phase = m.negative_phase.clone();
    // Harness
    if !raw {
        let mut incs: Vec<String> = vec!["assert.js".into(), "sta.js".into()];
        if is_async {
            incs.push("doneprintHandle.js".into());
        }
        for i in &m.includes {
            if !incs.contains(i) {
                incs.push(i.clone());
            }
        }
        for i in incs {
            let text = match h.files.get(&i) {
                Some(t) => t.clone(),
                None => return Err(format!("missing harness file {}", i)),
            };
            let text = if strict { format!("\"use strict\";\n{}", text) } else { text };
            if let Err(e) = vm.run_script_str(&text) {
                let msg = vm.error_string(&e);
                return Err(format!("harness {} failed: {}", i, msg));
            }
        }
    }
    let result: Result<Value, Value> = if module {
        let name = path.to_string_lossy().to_string();
        match vm.module_from_source(js_core::string::JsStr::from_str(&name), src) {
            Err(e) => Err(e),
            Ok(md) => match vm.load_requested(md).and_then(|_| vm.link_module(md)) {
                Err(e) => Err(e),
                Ok(()) => match vm.evaluate_module(md) {
                    Err(e) => Err(e),
                    Ok(p) => {
                        let _ = vm.run_jobs();
                        // A rejected evaluation promise is an uncaught error.
                        match &vm.heap.get(p).kind {
                            js_core::vm::Kind::Promise(pd) if pd.state == js_core::vm::PromiseState::Rejected => Err(pd.result.clone()),
                            _ => Ok(Value::Undefined),
                        }
                    }
                },
            },
        }
    } else {
        let text = if strict { format!("\"use strict\";\n{}", src) } else { src.to_string() };
        vm.run_script_str(&text)
    };
    let job_err = match &result {
        Ok(_) => match vm.run_jobs() {
            Ok(()) => None,
            Err(e) => Some(e),
        },
        Err(_) => None,
    };
    let result = match (result, job_err) {
        (Ok(_), Some(e)) => Err(e),
        (r, _) => r,
    };
    if vm.terminated {
        return Err(String::from("timeout (instruction budget exhausted)"));
    }
    match (result, &negative_type) {
        (Err(e), Some(t)) => {
            let name = error_name(&mut vm, &e);
            if &name == t {
                Ok(())
            } else {
                Err(format!("expected {} ({}), got {}", t, negative_phase.as_deref().unwrap_or(""), vm.error_string(&e)))
            }
        }
        (Err(e), None) => Err(format!("uncaught {}", vm.error_string(&e))),
        (Ok(_), Some(t)) => Err(format!("expected {} to be thrown", t)),
        (Ok(_), None) => {
            if is_async {
                let o = out.borrow();
                if o.contains("Test262:AsyncTestComplete") {
                    Ok(())
                } else if let Some(i) = o.find("Test262:AsyncTestFailure") {
                    Err(o[i..].lines().next().unwrap_or("").to_string())
                } else {
                    Err(String::from("async test did not complete"))
                }
            } else {
                Ok(())
            }
        }
    }
}

fn error_name(vm: &mut Vm, e: &Value) -> String {
    if let Value::Object(o) = e {
        if let Ok(c) = vm.get(*o, &js_core::vm::PropertyKey::from_str("constructor")) {
            if let Value::Object(co) = c {
                if let Ok(Value::String(n)) = vm.get(co, &js_core::vm::PropertyKey::from_str("name")) {
                    return n.to_rust();
                }
            }
        }
    }
    String::from("?")
}

pub fn run_test(h: &Harness, path: &Path, src: &str, m: &Meta) -> (Outcome, String) {
    let mut variants: Vec<bool> = Vec::new();
    if m.has_flag("module") || m.has_flag("raw") {
        variants.push(false);
    } else {
        if !m.has_flag("onlyStrict") {
            variants.push(false);
        }
        if !m.has_flag("noStrict") {
            variants.push(true);
        }
    }
    for strict in variants {
        if let Err(e) = run_one(h, path, src, m, strict) {
            return (Outcome::Fail, format!("[{}] {}", if strict { "strict" } else { "sloppy" }, e));
        }
    }
    (Outcome::Pass, String::new())
}

pub fn dir_key(rel: &str) -> String {
    let parts: Vec<&str> = rel.split('/').collect();
    if parts.len() >= 3 {
        format!("{}/{}", parts[0], parts[1])
    } else {
        parts[0].to_string()
    }
}

pub struct Report {
    pub per_dir: BTreeMap<String, (usize, usize, usize)>, // pass, fail, skip
    pub failures: Vec<(String, String)>,
}

pub fn run(cfg: &Config) -> Report {
    let files = Arc::new(collect(&cfg.root, &cfg.filters));
    let harness = Arc::new(Harness::load(&cfg.root));
    let idx = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<Vec<(String, Outcome, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for _ in 0..cfg.threads {
        let files = files.clone();
        let idx = idx.clone();
        let results = results.clone();
        let root = cfg.root.clone();
        let parse_only = cfg.parse_only;
        let harness = harness.clone();
        let h = std::thread::Builder::new()
            .stack_size(256 << 20)
            .spawn(move || loop {
                let i = idx.fetch_add(1, Ordering::Relaxed);
                if i >= files.len() {
                    break;
                }
                let p = &files[i];
                let rel = p.strip_prefix(root.join("test")).unwrap().to_string_lossy().to_string();
                let src = match std::fs::read_to_string(p) {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let m = parse_meta(&src);
                let (o, d) = if m.features.iter().any(|f| OUT_OF_SCOPE.contains(&f.as_str())) {
                    (Outcome::Skip, String::from("out of scope feature"))
                } else if m.has_flag("CanBlockIsTrue") {
                    // This host's main agent has [[CanBlock]] false (INTERPRETING.md).
                    (Outcome::Skip, String::from("host [[CanBlock]] is false"))
                } else if parse_only {
                    match std::panic::catch_unwind(|| run_parse(&src, &m)) {
                        Ok(r) => r,
                        Err(_) => (Outcome::Fail, String::from("panic")),
                    }
                } else {
                    let h = &harness;
                    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run_test(h, p, &src, &m))) {
                        Ok(r) => r,
                        Err(e) => {
                            let msg = e.downcast_ref::<String>().cloned().or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string())).unwrap_or_default();
                            (Outcome::Fail, format!("panic: {}", msg))
                        }
                    }
                };
                results.lock().unwrap().push((rel, o, d));
            })
            .unwrap();
        handles.push(h);
    }
    for h in handles {
        let _ = h.join();
    }
    let mut res = Arc::try_unwrap(results).ok().unwrap().into_inner().unwrap();
    res.sort_by(|a, b| a.0.cmp(&b.0));
    let mut per_dir: BTreeMap<String, (usize, usize, usize)> = BTreeMap::new();
    let mut failures = Vec::new();
    for (rel, o, d) in res {
        let e = per_dir.entry(dir_key(&rel)).or_insert((0, 0, 0));
        match o {
            Outcome::Pass => e.0 += 1,
            Outcome::Fail => {
                e.1 += 1;
                failures.push((rel, d));
            }
            Outcome::Skip => e.2 += 1,
        }
    }
    Report { per_dir, failures }
}

pub fn print_table(r: &Report) {
    let (mut tp, mut tf, mut ts) = (0, 0, 0);
    println!("{:<48} {:>7} {:>7} {:>7} {:>8}", "directory", "pass", "total", "skip", "pct");
    for (k, (p, f, s)) in &r.per_dir {
        let total = p + f;
        tp += p;
        tf += f;
        ts += s;
        let pct = if total > 0 { 100.0 * *p as f64 / total as f64 } else { 100.0 };
        println!("{:<48} {:>7} {:>7} {:>7} {:>7.2}%", k, p, total, s, pct);
    }
    let total = tp + tf;
    println!("{:<48} {:>7} {:>7} {:>7} {:>7.2}%", "TOTAL", tp, total, ts, 100.0 * tp as f64 / total.max(1) as f64);
}

pub fn main(args: Vec<String>) -> i32 {
    let mut root = None;
    let mut parse_only = false;
    let mut verbose = false;
    let mut json = None;
    let mut filters = Vec::new();
    let mut threads = 4;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--parse" => parse_only = true,
            "-v" | "--verbose" => verbose = true,
            "--json" => json = it.next().map(PathBuf::from),
            "-j" => threads = it.next().and_then(|x| x.parse().ok()).unwrap_or(4),
            _ if root.is_none() => root = Some(PathBuf::from(a)),
            _ => filters.push(a),
        }
    }
    let root = match root {
        Some(r) => r,
        None => {
            eprintln!("usage: js_core-test262 <test262-dir> [--parse] [-v] [filters…]");
            return 2;
        }
    };
    let cfg = Config { root, parse_only, filters, verbose, json, threads };
    let r = run(&cfg);
    if cfg.verbose {
        for (f, d) in &r.failures {
            println!("FAIL {} — {}", f, d);
        }
    }
    print_table(&r);
    if let Some(j) = &cfg.json {
        let mut s = String::from("{\n");
        for (i, (k, (p, f, sk))) in r.per_dir.iter().enumerate() {
            s.push_str(&format!("  \"{}\": [{}, {}, {}]{}\n", k, p, p + f, sk, if i + 1 < r.per_dir.len() { "," } else { "" }));
        }
        s.push_str("}\n");
        let _ = std::fs::write(j, s);
    }
    0
}
