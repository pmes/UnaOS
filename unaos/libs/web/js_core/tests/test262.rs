//! JSCORE gate: the official ECMAScript conformance suite (test262), pinned at a commit, fetched at test time.
//!
//! The checkout is found at `$JS_TEST262_DIR`, else `<target>/test262` (cloned shallow at the pinned commit on
//! first use). Offline (or `JS_OFFLINE=1`), the test is skipped. The suite is never placed in the source tree.

#[path = "t262/meta.rs"]
mod meta;
#[path = "t262/host.rs"]
mod host;
#[path = "t262/runner.rs"]
mod runner;

use std::path::PathBuf;
use std::process::Command;

pub const TEST262_REPO: &str = "https://github.com/tc39/test262.git";
pub const TEST262_COMMIT: &str = "7ab7fafa0003f73fc85c1b95d88094d33f7eb8bd";

fn locate() -> Option<PathBuf> {
    if std::env::var("JS_OFFLINE").is_ok() {
        return None;
    }
    if let Ok(d) = std::env::var("JS_TEST262_DIR") {
        let p = PathBuf::from(d);
        if p.join("test").is_dir() {
            return Some(p);
        }
    }
    let target = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../../target/test262");
    if target.join("test").is_dir() {
        return Some(target);
    }
    let ok = Command::new("git").args(["init", "-q"]).arg(&target).status().map(|s| s.success()).unwrap_or(false)
        && Command::new("git").arg("-C").arg(&target).args(["fetch", "-q", "--depth", "1", TEST262_REPO, TEST262_COMMIT]).status().map(|s| s.success()).unwrap_or(false)
        && Command::new("git").arg("-C").arg(&target).args(["checkout", "-q", "FETCH_HEAD"]).status().map(|s| s.success()).unwrap_or(false);
    if ok { Some(target) } else { None }
}

#[test]
fn test262_parse() {
    let Some(root) = locate() else {
        eprintln!("test262 unavailable (offline): skipped");
        return;
    };
    let cfg = runner::Config { root, parse_only: true, filters: Vec::new(), verbose: false, json: None, threads: 4 };
    let r = runner::run(&cfg);
    runner::print_table(&r);
    for (f, d) in r.failures.iter().take(50) {
        eprintln!("FAIL {} — {}", f, d);
    }
    assert!(r.failures.is_empty(), "{} parse failures", r.failures.len());
}
