// SPDX-License-Identifier: LGPL-3.0-or-later
// M3: the index (v2/v3/v4 + extensions) byte-identical round trip, checkout, status, write-tree
// and commit creation against git.
mod common;

use git_core::index::Index;
use git_core::{HashKind, ObjectId, Repository, Signature};

fn hex(o: Vec<u8>) -> String {
    String::from_utf8_lossy(&o).trim().to_string()
}

fn seed(dir: &std::path::Path) {
    common::git(dir, &["init", "-q", "-b", "main"]);
    for (p, d) in [("a.txt", "alpha\n"), ("dir/b.txt", "beta\n"), ("dir/sub/c.txt", "gamma\n"), ("x-y.txt", "xy\n"), ("dir.txt", "d\n"), ("very/deep/nested/path/to/a/file/with/a/long/name.rs", "fn main() {}\n")] {
        let f = dir.join(p);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(f, d).unwrap();
    }
    std::fs::write(dir.join("run.sh"), "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(dir.join("run.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    std::os::unix::fs::symlink("a.txt", dir.join("link")).unwrap();
    common::git(dir, &["add", "-A"]);
    common::git(dir, &["commit", "-q", "-m", "seed"]);
}

#[test]
fn index_versions_roundtrip() {
    let dir = common::scratch("index");
    seed(&dir);
    let check = |label: &str| -> Index {
        let raw = std::fs::read(dir.join(".git/index")).unwrap();
        let idx = Index::parse(HashKind::Sha1, &raw).unwrap();
        assert_eq!(idx.serialize(), raw, "{label}: byte-identical");
        // ls-files -s agreement
        let ls = String::from_utf8(common::git(&dir, &["ls-files", "-s"])).unwrap();
        let ours: String = idx.entries.iter().map(|e| format!("{:06o} {} {}\t{}\n", e.mode, e.oid(), e.stage(), String::from_utf8_lossy(&e.path))).collect();
        assert_eq!(ours, ls, "{label}: ls-files -s");
        idx
    };
    let v2 = check("v2 after commit");
    assert_eq!(v2.version, 2);
    let ct = v2.cache_tree().unwrap();
    assert_eq!(ct[0].id.unwrap().to_hex(), hex(common::git(&dir, &["rev-parse", "HEAD^{tree}"])), "TREE root");
    common::git(&dir, &["update-index", "--skip-worktree", "dir/b.txt"]);
    assert_eq!(check("v3 skip-worktree").version, 3);
    common::git(&dir, &["update-index", "--no-skip-worktree", "dir/b.txt"]);
    common::git(&dir, &["update-index", "--index-version", "4"]);
    assert_eq!(check("v4").version, 4);
    common::git(&dir, &["update-index", "--untracked-cache"]);
    common::git(&dir, &["status", "--porcelain"]);
    let idx = check("v4 + UNTR");
    let sigs: Vec<String> = idx.extensions.iter().map(|x| String::from_utf8_lossy(&x.sig).into_owned()).collect();
    println!("index extensions preserved: {sigs:?}");
}

#[test]
fn status_matches_git() {
    let dir = common::scratch("status");
    seed(&dir);
    std::fs::write(dir.join("a.txt"), "alpha changed\n").unwrap(); // " M"
    std::fs::write(dir.join("dir/b.txt"), "beta staged\n").unwrap();
    common::git(&dir, &["add", "dir/b.txt"]); // "M "
    std::fs::write(dir.join("dir/b.txt"), "beta staged then changed\n").unwrap(); // "MM"
    std::fs::remove_file(dir.join("x-y.txt")).unwrap(); // " D"
    common::git(&dir, &["rm", "-q", "--cached", "dir.txt"]); // "D " + "?? dir.txt"
    std::fs::write(dir.join("new.txt"), "n\n").unwrap();
    common::git(&dir, &["add", "new.txt"]); // "A "
    std::fs::create_dir_all(dir.join("untracked/deeper")).unwrap();
    std::fs::write(dir.join("untracked/deeper/u.txt"), "u").unwrap(); // "?? untracked/"
    std::fs::write(dir.join("dir/sub/loose.txt"), "l").unwrap(); // "?? dir/sub/loose.txt"
    std::fs::write(dir.join(".gitignore"), "*.log\nignored-dir/\n").unwrap(); // "?? .gitignore"
    std::fs::write(dir.join("x.log"), "ignored").unwrap();
    std::fs::create_dir_all(dir.join("ignored-dir")).unwrap();
    std::fs::write(dir.join("ignored-dir/f"), "i").unwrap();
    std::fs::create_dir_all(dir.join("only-ignored")).unwrap();
    std::fs::write(dir.join("only-ignored/a.log"), "i").unwrap();
    std::fs::set_permissions(dir.join("run.sh"), std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap(); // " M" mode
    let want = String::from_utf8(common::git(&dir, &["-c", "status.renames=false", "status", "--porcelain", "--untracked-files=normal"])).unwrap();
    let repo = Repository::discover(&dir).unwrap();
    let ours: String = repo.status(true).unwrap().iter().map(|e| format!("{}{} {}\n", e.staged as char, e.unstaged as char, String::from_utf8_lossy(&e.path))).collect();
    assert_eq!(ours, want);
    assert!(repo.is_dirty().unwrap());
    println!("status: {} entries identical to git status --porcelain", want.lines().count());
}

#[test]
fn checkout_then_git_sees_clean() {
    let src = common::scratch("co-src");
    seed(&src);
    let dst = common::scratch("co-dst");
    common::git(&dst, &["clone", "-q", "--no-checkout", src.to_str().unwrap(), "w"]);
    let w = dst.join("w");
    let repo = Repository::discover(&w).unwrap();
    let head = repo.head().unwrap().1.unwrap();
    let (_, d) = repo.read(&head).unwrap();
    let tree = git_core::Commit::parse(HashKind::Sha1, &d).unwrap().tree().unwrap();
    repo.checkout_tree(&tree).unwrap();
    assert!(!repo.is_dirty().unwrap(), "ours: clean after checkout");
    let st = common::git(&w, &["status", "--porcelain"]);
    assert!(st.is_empty(), "git status after our checkout: {}", String::from_utf8_lossy(&st));
    let df = common::git(&w, &["diff-index", "--cached", "HEAD"]);
    assert!(df.is_empty());
    assert!(std::fs::symlink_metadata(w.join("link")).unwrap().file_type().is_symlink());
}

#[test]
fn commit_identical_to_git_commit() {
    // Two identical repositories; git commits in one, git_core in the other.
    let a = common::scratch("commit-a");
    let b = common::scratch("commit-b");
    seed(&a);
    seed(&b);
    for d in [&a, &b] {
        std::fs::write(d.join("a.txt"), "alpha 2\n").unwrap();
        std::fs::write(d.join("dir/new.txt"), "new\n").unwrap();
        common::git(d, &["add", "-A"]);
    }
    common::git(&a, &["commit", "-q", "-m", "second commit\n\nwith a body"]);
    let repo = Repository::discover(&b).unwrap();
    let wt = repo.write_tree(&repo.index().unwrap()).unwrap();
    assert_eq!(wt.to_hex(), hex(common::git(&b, &["write-tree"])), "write-tree");
    let author = Signature::new(b"A U Thor", b"author@example.com", 1700000000, 90);
    let committer = Signature::new(b"C O Mitter", b"committer@example.com", 1700000100, -300);
    let id = repo.commit_as("second commit\n\nwith a body", &author, &committer).unwrap();
    assert_eq!(id.to_hex(), hex(common::git(&a, &["rev-parse", "HEAD"])), "commit id equals git commit's");
    assert_eq!(hex(common::git(&b, &["rev-parse", "HEAD"])), id.to_hex(), "branch advanced");
    // Reflogs agree byte for byte (HEAD and the branch).
    for log in ["logs/HEAD", "logs/refs/heads/main"] {
        let la = std::fs::read(a.join(".git").join(log)).unwrap();
        let lb = std::fs::read(b.join(".git").join(log)).unwrap();
        assert_eq!(String::from_utf8_lossy(&lb), String::from_utf8_lossy(&la), "{log}");
    }
    let f = common::git_raw(&b, &["fsck", "--strict"], None);
    assert!(f.status.success());
    // rev-parse forms
    for spec in ["HEAD", "HEAD~1", "HEAD^", "HEAD^{tree}", "main", "HEAD:dir/new.txt", &id.to_hex()[..9]] {
        assert_eq!(repo.rev_parse(spec).unwrap().to_hex(), hex(common::git(&b, &["rev-parse", spec])), "rev-parse {spec}");
    }
    let _ = ObjectId::from_hex(b"0000000000000000000000000000000000000000");
}
