//! The test262 driver shared by the `js_core-test262` example and the `cargo test` gate.

use super::meta::{parse_meta, Meta, OUT_OF_SCOPE};
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
    let idx = Arc::new(AtomicUsize::new(0));
    let results: Arc<Mutex<Vec<(String, Outcome, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let mut handles = Vec::new();
    for _ in 0..cfg.threads {
        let files = files.clone();
        let idx = idx.clone();
        let results = results.clone();
        let root = cfg.root.clone();
        let parse_only = cfg.parse_only;
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
                } else if parse_only {
                    match std::panic::catch_unwind(|| run_parse(&src, &m)) {
                        Ok(r) => r,
                        Err(_) => (Outcome::Fail, String::from("panic")),
                    }
                } else {
                    (Outcome::Skip, String::from("run mode not available"))
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
