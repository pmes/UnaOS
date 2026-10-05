// SPDX-License-Identifier: LGPL-3.0-or-later
// M1: loose objects. git_core writes blobs/trees/commits/tags as loose objects into a repository
// `git init` made; `git fsck --strict --full` must accept them and `git cat-file` must agree. The
// other direction: objects git writes decode and verify here, and a commit built by git_core with
// git's identity/date rules has the SAME id as `git commit-tree`'s. Both object formats.
mod common;

use std::path::Path;

use git_core::object::{mode, Kind, Tree, TreeEntry};
use git_core::{loose, Commit, HashKind, ObjectId, Signature, Tag};

fn write_loose(repo: &Path, hk: HashKind, kind: Kind, payload: &[u8]) -> ObjectId {
    let (id, bytes) = loose::encode(hk, kind, payload, 6);
    let p = repo.join(".git/objects").join(loose::path(&id));
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, bytes).unwrap();
    id
}

fn run(hk: HashKind) {
    let dir = common::scratch(&format!("loose-{}", hk.name()));
    common::git(&dir, &["init", "-q", "-b", "main", &format!("--object-format={}", hk.name())]);
    // Blobs: text, binary, empty.
    let b1 = write_loose(&dir, hk, Kind::Blob, b"hello\nworld\n");
    let b2 = write_loose(&dir, hk, Kind::Blob, &common::prng(5, 70_000));
    let b3 = write_loose(&dir, hk, Kind::Blob, b"");
    let mut sub = Tree { entries: vec![TreeEntry { mode: mode::BLOB, name: b"empty".to_vec(), id: b3 }] };
    sub.sort();
    let t_sub = write_loose(&dir, hk, Kind::Tree, &sub.serialize());
    let mut root = Tree {
        entries: vec![
            TreeEntry { mode: mode::BLOB, name: b"a.txt".to_vec(), id: b1 },
            TreeEntry { mode: mode::BLOB_EXEC, name: b"bin".to_vec(), id: b2 },
            TreeEntry { mode: mode::TREE, name: b"a".to_vec(), id: t_sub },
            TreeEntry { mode: mode::LINK, name: b"link".to_vec(), id: b1 },
        ],
    };
    root.sort();
    let names: Vec<_> = root.entries.iter().map(|e| String::from_utf8_lossy(&e.name).into_owned()).collect();
    assert_eq!(names, ["a.txt", "a", "bin", "link"]); // "a.txt" < "a/"
    let t = write_loose(&dir, hk, Kind::Tree, &root.serialize());
    let author = Signature::new(b"A U Thor", b"author@example.com", 1700000000, 90);
    let committer = Signature::new(b"C O Mitter", b"committer@example.com", 1700000100, -300);
    let c = Commit::new(t, &[], &author, &committer, b"initial\n");
    let cid = write_loose(&dir, hk, Kind::Commit, &c.serialize());
    let tag = Tag::new(cid, Kind::Commit, b"v1", &committer, b"release\n");
    let tid = write_loose(&dir, hk, Kind::Tag, &tag.serialize());
    std::fs::write(dir.join(".git/refs/heads/main"), format!("{cid}\n")).unwrap();
    std::fs::write(dir.join(".git/refs/tags/v1"), format!("{tid}\n")).unwrap();

    let fsck = common::git_raw(&dir, &["fsck", "--strict", "--full", "--no-dangling"], None);
    assert!(fsck.status.success(), "fsck: {}{}", String::from_utf8_lossy(&fsck.stdout), String::from_utf8_lossy(&fsck.stderr));
    assert!(fsck.stdout.is_empty() && fsck.stderr.is_empty(), "fsck said: {}", String::from_utf8_lossy(&fsck.stderr));

    // git's commit-tree with the same identity/dates yields the same id (exact encoding rules).
    let git_c = common::git_raw(&dir, &["commit-tree", &t.to_hex()], Some(b"initial\n"));
    assert!(git_c.status.success());
    assert_eq!(String::from_utf8_lossy(&git_c.stdout).trim(), cid.to_hex(), "commit id vs git commit-tree");
    // git's mktag agrees on the tag.
    let git_t = common::git_raw(&dir, &["mktag"], Some(&tag.serialize()));
    assert!(git_t.status.success(), "{}", String::from_utf8_lossy(&git_t.stderr));
    assert_eq!(String::from_utf8_lossy(&git_t.stdout).trim(), tid.to_hex());
    // ls-tree agrees.
    let ls = common::git(&dir, &["ls-tree", &t.to_hex()]);
    assert!(String::from_utf8_lossy(&ls).contains(&format!("040000 tree {t_sub}\ta\n")));

    // The other direction: objects git writes.
    std::fs::write(dir.join("f"), b"git wrote this\n").unwrap();
    let gid = common::git(&dir, &["hash-object", "-w", "f"]);
    let gid = ObjectId::from_hex(String::from_utf8_lossy(&gid).trim().as_bytes()).unwrap();
    let file = std::fs::read(dir.join(".git/objects").join(loose::path(&gid))).unwrap();
    let (k, p) = loose::decode_verified(&gid, &file, 1 << 20).unwrap();
    assert_eq!((k, p.as_slice()), (Kind::Blob, &b"git wrote this\n"[..]));
    // A real commit git made (with a parent).
    std::fs::write(dir.join("g"), b"second\n").unwrap();
    common::git(&dir, &["add", "g"]);
    common::git(&dir, &["commit", "-q", "-m", "second"]);
    let head = common::git(&dir, &["rev-parse", "HEAD"]);
    let hid = ObjectId::from_hex(String::from_utf8_lossy(&head).trim().as_bytes()).unwrap();
    let file = std::fs::read(dir.join(".git/objects").join(loose::path(&hid))).unwrap();
    let (k, p) = loose::decode_verified(&hid, &file, 1 << 20).unwrap();
    assert_eq!(k, Kind::Commit);
    let gc = Commit::parse(hk, &p).unwrap();
    assert_eq!(gc.serialize(), p);
    assert_eq!(gc.parents(), vec![cid]);
    // Rebuild that commit from its parsed fields — identical bytes.
    let rebuilt = Commit::new(gc.tree().unwrap(), &gc.parents(), &gc.author().unwrap(), &gc.committer().unwrap(), gc.message());
    assert_eq!(rebuilt.serialize(), p);
    println!("{}: fsck clean, commit-tree/mktag ids equal, git objects verified", hk.name());
}

#[test]
fn sha1_repo() {
    run(HashKind::Sha1);
}

#[test]
fn sha256_repo() {
    run(HashKind::Sha256);
}
