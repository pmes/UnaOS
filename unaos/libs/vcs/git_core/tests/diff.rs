// SPDX-License-Identifier: LGPL-3.0-or-later
// M3: `git diff A B` and `git diff --stat A B` byte-equal on 200 commit pairs of this repository's
// own history (a full bare clone of itself), plus histogram diff, plus targeted rename/binary/mode
// fixtures.
mod common;

use std::path::Path;

use git_core::diff::{self, Algorithm, ObjectSource, PatchOptions, RenameOptions};
use git_core::{Commit, Repository};

fn ours(repo: &Repository, a: &str, b: &str, stat: bool, alg: Algorithm) -> Vec<u8> {
    let ta = tree_of(repo, a);
    let tb = tree_of(repo, b);
    let pairs = diff::tree_changes(repo, Some(&ta), Some(&tb)).unwrap();
    let pairs = diff::detect_renames(repo, pairs, &RenameOptions::default()).unwrap();
    let opt = PatchOptions { algorithm: alg, ..Default::default() };
    let mut out = Vec::new();
    if stat {
        diff::stat(repo, &pairs, &opt, 80, &mut out).unwrap();
    } else {
        diff::patch(repo, &pairs, &opt, &mut out).unwrap();
    }
    out
}

fn tree_of(repo: &Repository, rev: &str) -> git_core::ObjectId {
    let id = repo.rev_parse(rev).unwrap();
    let (_, d) = repo.read_object(&id).unwrap();
    Commit::parse(repo.hash, &d).unwrap().tree().unwrap()
}

fn first_difference(a: &[u8], b: &[u8]) -> String {
    let al: Vec<&[u8]> = a.split(|&c| c == b'\n').collect();
    let bl: Vec<&[u8]> = b.split(|&c| c == b'\n').collect();
    for i in 0..al.len().max(bl.len()) {
        let x = al.get(i).copied().unwrap_or(b"<EOF>");
        let y = bl.get(i).copied().unwrap_or(b"<EOF>");
        if x != y {
            return format!("line {}: git={:?} ours={:?}", i + 1, String::from_utf8_lossy(x), String::from_utf8_lossy(y));
        }
    }
    "identical?".into()
}

fn compare_pairs(repo_dir: &Path, repo: &Repository, pairs: &[(String, String)], alg: Algorithm, flag: Option<&str>) -> (usize, usize, Vec<String>) {
    let (mut ok, mut bytes) = (0, 0);
    let mut bad = Vec::new();
    for (p, c) in pairs {
        for stat in [false, true] {
            let mut args = vec!["diff"];
            if let Some(f) = flag {
                args.push(f);
            }
            if stat {
                args.push("--stat");
            }
            args.push(p);
            args.push(c);
            let want = common::git(repo_dir, &args);
            let got = ours(repo, p, c, stat, alg);
            bytes += want.len();
            if want == got {
                ok += 1;
            } else {
                bad.push(format!("{} {p}..{c}: {}", if stat { "stat " } else { "patch" }, first_difference(&want, &got)));
            }
        }
    }
    (ok, bytes, bad)
}

fn self_clone() -> Option<std::path::PathBuf> {
    let dir = common::scratch("diffclone");
    let url = format!("file://{}", common::self_repo().display());
    let o = common::git_raw(&dir, &["clone", "-q", "--bare", "--no-local", "--single-branch", &url, "full.git"], None);
    if !o.status.success() {
        println!("SKIPPED (clone of self failed: {})", String::from_utf8_lossy(&o.stderr));
        return None;
    }
    Some(dir.join("full.git"))
}

#[test]
fn diff_200_pairs_of_self() {
    let Some(repo_dir) = self_clone() else { return };
    let repo = Repository::open(&repo_dir).unwrap();
    let n: usize = std::env::var("GITCORE_DIFF_PAIRS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let revs = common::git(&repo_dir, &["rev-list", "--first-parent", "--parents", &format!("--max-count={}", n * 2), "HEAD"]);
    let mut pairs = Vec::new();
    for l in String::from_utf8_lossy(&revs).lines() {
        let v: Vec<&str> = l.split(' ').collect();
        if v.len() >= 2 {
            pairs.push((v[1].to_string(), v[0].to_string()));
        }
        if pairs.len() == n {
            break;
        }
    }
    let t = std::time::Instant::now();
    let (ok, bytes, bad) = compare_pairs(&repo_dir, &repo, &pairs, Algorithm::Myers, None);
    println!("myers: {ok}/{} outputs byte-equal ({} pairs x patch+stat, {bytes} bytes of git output) in {:?}", pairs.len() * 2, pairs.len(), t.elapsed());
    for b in bad.iter().take(12) {
        println!("  MISMATCH {b}");
    }
    let hn = (n / 4).max(1).min(pairs.len());
    let (hok, _, hbad) = compare_pairs(&repo_dir, &repo, &pairs[..hn], Algorithm::Histogram, Some("--histogram"));
    println!("histogram: {hok}/{} outputs byte-equal", hn * 2);
    for b in hbad.iter().take(6) {
        println!("  MISMATCH {b}");
    }
    assert!(bad.is_empty() && hbad.is_empty(), "{} myers + {} histogram mismatches", bad.len(), hbad.len());
}

/// Renames (exact, basename, matrix), binary files, mode changes, type changes, quoting,
/// no-newline-at-EOF, empty files — the corners the history may not hit.
#[test]
fn diff_fixture_corners() {
    let dir = common::scratch("diffcorners");
    common::git(&dir, &["init", "-q", "-b", "main"]);
    let w = |p: &str, d: &[u8]| {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, d).unwrap();
    };
    let text: String = (0..60).map(|i| format!("line number {i} with some content\n")).collect();
    w("keep/exact.txt", text.as_bytes());
    w("old/basename.rs", text.as_bytes());
    w("matrix_src.txt", format!("{text}{text}").as_bytes());
    w("bin.dat", &[0u8, 1, 2, 3, 0, 9]);
    w("mode.sh", b"#!/bin/sh\necho hi\n");
    w("noeol.txt", b"a\nb\nc");
    w("type.txt", b"was a file\n");
    w("sp ace.txt", b"x\n");
    w("ünï.txt", b"u\n");
    w("del.txt", b"deleted\n");
    w("empty", b"");
    common::git(&dir, &["add", "-A"]);
    common::git(&dir, &["commit", "-q", "-m", "one"]);
    let _ = std::fs::remove_file(dir.join("keep/exact.txt"));
    w("moved/exact.txt", text.as_bytes());
    let _ = std::fs::remove_file(dir.join("old/basename.rs"));
    w("new/basename.rs", text.replace("line number 5 ", "LINE 5 ").as_bytes());
    let _ = std::fs::remove_file(dir.join("matrix_src.txt"));
    w("other_name.txt", format!("{text}{}", text.replace("content", "stuff")).as_bytes());
    w("bin.dat", &[0u8, 1, 2, 3, 0, 9, 9, 9]);
    std::fs::set_permissions(dir.join("mode.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    w("noeol.txt", b"a\nB\nc");
    std::fs::remove_file(dir.join("type.txt")).unwrap();
    std::os::unix::fs::symlink("noeol.txt", dir.join("type.txt")).unwrap();
    w("sp ace.txt", b"y\n");
    w("ünï.txt", b"v\n");
    std::fs::remove_file(dir.join("del.txt")).unwrap();
    w("new-empty", b"");
    w("empty", b"now has content\n");
    common::git(&dir, &["add", "-A"]);
    common::git(&dir, &["commit", "-q", "-m", "two"]);
    let repo = Repository::discover(&dir).unwrap();
    for stat in [false, true] {
        let mut args = vec!["diff"];
        if stat {
            args.push("--stat");
        }
        args.extend(["HEAD~", "HEAD"]);
        let want = common::git(&dir, &args);
        let got = ours(&repo, "HEAD~", "HEAD", stat, Algorithm::Myers);
        assert!(want == got, "{}\n--- git ---\n{}\n--- ours ---\n{}", first_difference(&want, &got), String::from_utf8_lossy(&want), String::from_utf8_lossy(&got));
    }
}
