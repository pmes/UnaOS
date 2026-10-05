// SPDX-License-Identifier: LGPL-3.0-or-later
// Shared test helpers: a scratch directory under cargo's per-test tmp dir and the `git` oracle.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static N: AtomicUsize = AtomicUsize::new(0);

/// A fresh empty directory.
pub fn scratch(tag: &str) -> PathBuf {
    let base = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("git_core");
    let d = base.join(format!("{tag}-{}-{}", std::process::id(), N.fetch_add(1, Ordering::SeqCst)));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Run git in `dir` with a hermetic environment; returns stdout, panics on failure.
pub fn git(dir: &Path, args: &[&str]) -> Vec<u8> {
    let out = git_raw(dir, args, None);
    assert!(out.status.success(), "git {:?} failed: {}", args, String::from_utf8_lossy(&out.stderr));
    out.stdout
}

/// Run git; return the whole Output.
pub fn git_raw(dir: &Path, args: &[&str], stdin: Option<&[u8]>) -> std::process::Output {
    use std::io::Write;
    let mut c = Command::new("git");
    c.args(args)
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join(".xdg-none"))
        .env("GIT_AUTHOR_NAME", "A U Thor")
        .env("GIT_AUTHOR_EMAIL", "author@example.com")
        .env("GIT_COMMITTER_NAME", "C O Mitter")
        .env("GIT_COMMITTER_EMAIL", "committer@example.com")
        .env("GIT_AUTHOR_DATE", "1700000000 +0130")
        .env("GIT_COMMITTER_DATE", "1700000100 -0500")
        .env("LC_ALL", "C")
        .env("TZ", "UTC")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut ch = c.spawn().expect("git must be installed (the oracle)");
    if let Some(s) = stdin {
        ch.stdin.take().unwrap().write_all(s).unwrap();
    } else {
        drop(ch.stdin.take());
    }
    ch.wait_with_output().unwrap()
}

/// Deterministic pseudo-random bytes (xorshift).
pub fn prng(seed: u64, n: usize) -> Vec<u8> {
    let mut x = seed | 1;
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x as u8
        })
        .collect()
}

/// The repository this test is built from (the worktree root), for self-history tests.
pub fn self_repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().unwrap()
}
